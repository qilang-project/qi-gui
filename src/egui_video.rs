//! 窗口内视频播放 FFI
//!
//! 解码在 `video.rs`（ffmpeg 子进程 + 后台线程）；本文件管播放状态、音画同步和上屏：
//!
//! - **钟**：有声音时以声音为准 —— rodio 取走了多少采样，画面就放到哪儿；
//!   声音放完了（或者本来就没有）就接着走墙钟。每次读声音钟都把墙钟对齐过去，
//!   两者切换时不跳。
//! - **选帧**：每次 `视频画` 从通道里取出所有"已经到点"的帧，只留最新的一帧上传成
//!   纹理，前面没来得及显示的计为掉帧。还没到点的那一帧留在 `pending` 里等。
//! - **上屏**：纹理每换一帧，egui 就下发一次纹理增量，静止跳帧自然不会吞掉它；
//!   播放期间帧循环也不进静止档（见 `any_playing`），节奏不会被拖到 30Hz。
//!
//! 句柄、状态都在主线程 thread_local（纹理要 egui Context，rodio 输出流不是 Send）。

use crate::egui_app::{cstr, with_top_ui};
use crate::video::{self, AudioFeed, AudioProgress, Frame, Probe, VideoDecoder};
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::raw::c_char;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

use egui::{pos2, vec2, Color32, ColorImage, Rect, Sense, Shape, Stroke, TextureOptions};

/// 画面落后这么多就不再一帧帧追，直接让解码器跳过去
const RESYNC_MS: f64 = 1500.0;
/// 两次重启解码器的最小间隔：拖进度条时每帧都在跳，每帧起一个 ffmpeg 吃不消
const SEEK_THROTTLE: Duration = Duration::from_millis(120);

/// 墙钟：base 起，since 为 None 表示停着
#[derive(Clone, Copy)]
struct Clock {
    base_ms: f64,
    since: Option<Instant>,
}

impl Clock {
    fn now(&self) -> f64 {
        self.base_ms
            + self
                .since
                .map(|t| t.elapsed().as_secs_f64() * 1000.0)
                .unwrap_or(0.0)
    }
    fn set(&mut self, ms: f64, running: bool) {
        self.base_ms = ms;
        self.since = running.then(Instant::now);
    }
}

/// 一段声音：从 start_ms 起的 PCM 流挂在一个 sink 上。跳转时整段换掉
struct AudioOut {
    // 声明顺序即析构顺序：先停 sink，再杀 ffmpeg
    sink: rodio::Sink,
    _feed: AudioFeed,
    progress: AudioProgress,
    start_ms: f64,
}

impl AudioOut {
    fn start(handle: &rodio::OutputStreamHandle, path: &str, start_ms: f64) -> Option<Self> {
        let (feed, source, progress) = video::spawn_audio(path, start_ms)?;
        let sink = rodio::Sink::try_new(handle).ok()?;
        sink.append(source);
        Some(AudioOut {
            sink,
            _feed: feed,
            progress,
            start_ms,
        })
    }

    fn ms(&self) -> f64 {
        self.start_ms + self.progress.ms()
    }
}

#[derive(Default)]
struct Stats {
    /// 上传成纹理、真正显示过的帧
    shown: u64,
    /// 解出来了但被更新的帧顶掉、没显示过的帧
    dropped: u64,
    /// 显示时比应显示时刻晚了多少（毫秒），累计/最大
    late_sum: f64,
    late_max: f64,
    /// 解码器（重新）启动次数，含首次
    restarts: u64,
    /// 播放时真的用上了声音钟（有声音流、也打开了输出设备）
    audio_clock: bool,
}

struct Video {
    path: String,
    info: Probe,
    w: u32,
    h: u32,
    decoder: Option<VideoDecoder>,
    pending: Option<Frame>,
    ready: Option<Frame>,
    video_eof: bool,
    need_first: bool,
    /// 最近取出的那一帧的时刻；判断"最后一帧显示够了没有"用
    last_pts: f64,
    texture: Option<egui::TextureHandle>,
    audio: Option<AudioOut>,
    /// 声音输出设备：第一次出声时打开，跳转换 sink 不换设备（排在 audio 后面先析构 sink）
    device: Option<(rodio::OutputStream, rodio::OutputStreamHandle)>,
    /// 还没执行的跳转目标（被节流挡住的）
    seek_pending: Option<f64>,
    last_restart: Instant,
    clock: Clock,
    playing: bool,
    ended: bool,
    stats: Stats,
}

impl Video {
    fn open(path: &str) -> Option<Self> {
        let info = video::probe(path)?;
        let (w, h) = video::output_size(info.width, info.height);
        let decoder = VideoDecoder::spawn(path, 0.0, w, h, info.fps)?;
        Some(Video {
            path: path.to_string(),
            info,
            w,
            h,
            decoder: Some(decoder),
            pending: None,
            ready: None,
            video_eof: false,
            need_first: true,
            last_pts: f64::NEG_INFINITY,
            texture: None,
            audio: None,
            device: None,
            seek_pending: None,
            last_restart: Instant::now(),
            clock: Clock {
                base_ms: 0.0,
                since: None,
            },
            playing: false,
            ended: false,
            stats: Stats {
                restarts: 1,
                ..Stats::default()
            },
        })
    }

    fn duration(&self) -> f64 {
        self.info.duration_ms as f64
    }

    /// 当前播放位置（毫秒）
    fn position(&mut self) -> f64 {
        if self.ended {
            return self.duration();
        }
        if self.playing {
            if let Some(a) = &self.audio {
                if !a.progress.is_ended() {
                    self.stats.audio_clock = true;
                    let ms = a.ms();
                    self.clock.set(ms, true);
                    return ms;
                }
            }
        }
        let d = self.duration();
        let now = self.clock.now();
        if d > 0.0 {
            now.min(d)
        } else {
            now
        }
    }

    fn restart_decoder(&mut self, ms: f64) {
        self.last_restart = Instant::now();
        self.stats.restarts += 1;
        self.decoder = None; // 先杀旧的
        self.decoder = VideoDecoder::spawn(&self.path, ms, self.w, self.h, self.info.fps);
        self.pending = None;
        self.video_eof = self.decoder.is_none();
        self.need_first = true;
        self.last_pts = f64::NEG_INFINITY;
    }

    fn start_audio(&mut self, ms: f64) {
        self.audio = None;
        if !self.info.has_audio {
            return;
        }
        if self.device.is_none() {
            self.device = rodio::OutputStream::try_default().ok();
        }
        if let Some((_, handle)) = &self.device {
            self.audio = AudioOut::start(handle, &self.path, ms);
        }
    }

    /// 跳转：位置立刻变（进度条跟手），解码器按节流重启
    fn seek(&mut self, ms: f64) {
        let d = self.duration();
        let ms = if d > 0.0 {
            ms.clamp(0.0, d)
        } else {
            ms.max(0.0)
        };
        if !self.playing && !self.ended && (self.position() - ms).abs() < 0.5 {
            return; // 按住进度条不动：同一个位置不必重启
        }
        self.ended = false;
        self.audio = None; // 旧位置的声音马上停
        self.clock.set(ms, self.playing);
        self.seek_pending = Some(ms);
        self.apply_seek(false);
    }

    fn apply_seek(&mut self, force: bool) {
        if self.seek_pending.is_none() || (!force && self.last_restart.elapsed() < SEEK_THROTTLE) {
            return;
        }
        self.seek_pending = None;
        // 节流期间如果在播放，钟已经往前走了，从现在的位置起
        let at = self.clock.now();
        self.restart_decoder(at);
        if self.playing {
            self.start_audio(at);
        }
        self.clock.set(at, self.playing);
    }

    fn play(&mut self) {
        if self.ended {
            self.seek(0.0);
        }
        self.apply_seek(true);
        if self.playing {
            return;
        }
        let pos = self.position();
        self.playing = true;
        if self.audio.is_none() {
            self.start_audio(pos);
        }
        if let Some(a) = &self.audio {
            a.sink.play();
        }
        self.clock.set(pos, true);
    }

    fn pause(&mut self) {
        if !self.playing {
            return;
        }
        let pos = self.position();
        self.playing = false;
        if let Some(a) = &self.audio {
            a.sink.pause();
        }
        self.clock.set(pos, false);
    }

    /// 从解码通道里取到点的帧；顺带判断放完没有
    fn tick(&mut self) {
        self.apply_seek(false);
        if self.seek_pending.is_some() {
            return; // 旧解码器出的都是跳转前的帧，等新的
        }
        let now = self.position();
        // 落后太多（比如有一阵没画）：别一帧帧追，让解码器直接跳到现在
        if self.playing && !self.need_first {
            if let Some(p) = &self.pending {
                if now - p.pts_ms > RESYNC_MS {
                    self.restart_decoder(now);
                }
            }
        }
        loop {
            if self.pending.is_none() {
                let Some(dec) = &self.decoder else { break };
                match dec.rx.try_recv() {
                    Ok(f) => self.pending = Some(f),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.video_eof = true;
                        break;
                    }
                }
            }
            let due = self
                .pending
                .as_ref()
                .is_some_and(|f| f.pts_ms <= now + 1.0 || self.need_first);
            if !due {
                break;
            }
            self.need_first = false;
            self.last_pts = self.pending.as_ref().map_or(0.0, |f| f.pts_ms);
            if self.ready.replace(self.pending.take().unwrap()).is_some() {
                self.stats.dropped += 1;
            }
        }
        if self.playing && self.video_eof && self.pending.is_none() && self.ready.is_none() {
            // 画面解到头、最后一帧也显示满一帧的时长、声音也放完了，才算放完
            let audio_done = self.audio.as_ref().is_none_or(|a| a.progress.is_ended());
            let d = self.duration();
            if audio_done && now >= self.last_pts + 1000.0 / self.info.fps {
                self.playing = false;
                self.ended = true;
                self.audio = None;
                self.clock.set(d, false);
            }
        }
    }

    /// 把待显示的帧上传成纹理
    fn upload(&mut self, ctx: &egui::Context, id: u64) {
        let Some(f) = self.ready.take() else { return };
        if self.playing {
            let late = (self.position() - f.pts_ms).max(0.0);
            self.stats.late_sum += late;
            self.stats.late_max = self.stats.late_max.max(late);
        }
        let img = ColorImage {
            size: [self.w as usize, self.h as usize],
            pixels: f
                .rgba
                .chunks_exact(4)
                .map(|p| Color32::from_rgb(p[0], p[1], p[2]))
                .collect(),
        };
        match &mut self.texture {
            Some(t) => t.set(img, TextureOptions::LINEAR),
            None => {
                self.texture =
                    Some(ctx.load_texture(format!("qi_video:{id}"), img, TextureOptions::LINEAR))
            }
        }
        self.stats.shown += 1;
    }

    fn report(&self) {
        if std::env::var("QI_GUI_STATS")
            .map(|v| v == "1")
            .unwrap_or(false)
        {
            let s = &self.stats;
            let avg = if s.shown > 0 {
                s.late_sum / s.shown as f64
            } else {
                0.0
            };
            eprintln!(
                "qi-gui 视频统计 {}：{}×{} @ {:.2}fps，显示 {} 帧，掉帧 {} 帧，\
                 平均晚 {:.1}ms，最多晚 {:.1}ms，解码器启动 {} 次，以{}为钟",
                self.path,
                self.w,
                self.h,
                self.info.fps,
                s.shown,
                s.dropped,
                avg,
                s.late_max,
                s.restarts,
                if s.audio_clock { "声音" } else { "墙钟" }
            );
        }
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        self.report();
    }
}

thread_local! {
    static VIDEOS: RefCell<HashMap<u64, Video>> = RefCell::new(HashMap::new());
    static NEXT_ID: RefCell<u64> = const { RefCell::new(1) };
}

fn with_video<R>(id: u64, f: impl FnOnce(&mut Video) -> R) -> Option<R> {
    VIDEOS.with(|vs| vs.borrow_mut().get_mut(&id).map(f))
}

/// 有视频正在播放：帧循环据此不进静止档（见 egui_app 的 `帧结束`）
pub(crate) fn any_playing() -> bool {
    VIDEOS.with(|vs| vs.borrow().values().any(|v| v.playing))
}

/// 关窗时调用：停掉所有视频、杀掉 ffmpeg
pub(crate) fn release_all() {
    let all: Vec<Video> = VIDEOS.with(|vs| vs.borrow_mut().drain().map(|(_, v)| v).collect());
    drop(all);
}

/// 本机有没有 ffmpeg / ffprobe
#[no_mangle]
pub extern "C" fn qi_gui_egui_video_available_impl() -> i64 {
    i64::from(video::find_tool("ffmpeg").is_some() && video::find_tool("ffprobe").is_some())
}

/// 视频加载(路径) → 句柄；没装 ffmpeg、文件打不开或没有画面都返回 0。
/// 加载后不播放，但会马上解出第一帧当封面
#[no_mangle]
pub extern "C" fn qi_gui_egui_video_load_impl(path: *const c_char) -> u64 {
    let p = cstr(path);
    if p.is_empty() {
        return 0;
    }
    let Some(v) = Video::open(&p) else {
        return 0;
    };
    let id = NEXT_ID.with(|n| {
        let mut n = n.borrow_mut();
        let id = *n;
        *n += 1;
        id
    });
    VIDEOS.with(|vs| vs.borrow_mut().insert(id, v));
    id
}

/// 画暂停时中间的播放钮
fn paint_play_badge(painter: &egui::Painter, rect: Rect) {
    let c = rect.center();
    let r = (rect.height().min(rect.width()) * 0.12).clamp(14.0, 30.0);
    painter.circle_filled(c, r, Color32::from_black_alpha(120));
    let s = r * 0.42;
    let tri = vec![
        pos2(c.x - s * 0.7, c.y - s),
        pos2(c.x - s * 0.7, c.y + s),
        pos2(c.x + s, c.y),
    ];
    painter.add(Shape::convex_polygon(tri, Color32::WHITE, Stroke::NONE));
}

/// 视频画(句柄, 最大宽) → 1 本帧画面被点了 / 0。按比例占满可用宽度（再不超过最大宽，
/// <=0 不限）；还没解出第一帧时画一块同尺寸的深色底，排版不跳
#[no_mangle]
pub extern "C" fn qi_gui_egui_video_draw_impl(id: u64, max_width: i64) -> i64 {
    VIDEOS.with(|vs| {
        let mut vs = vs.borrow_mut();
        let Some(v) = vs.get_mut(&id) else {
            return 0;
        };
        v.tick();
        with_top_ui(|ui| {
            v.upload(ui.ctx(), id);
            let mut w = ui.available_width();
            if max_width > 0 {
                w = w.min(max_width as f32);
            }
            let size = vec2(w, w * v.h as f32 / v.w.max(1) as f32);
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
            let painter = ui.painter();
            match &v.texture {
                Some(t) => painter.image(
                    t.id(),
                    rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                ),
                None => painter.rect_filled(rect, 0.0, Color32::from_gray(24)),
            };
            if !v.playing {
                paint_play_badge(painter, rect);
            }
            i64::from(resp.clicked())
        })
        .unwrap_or(0)
    })
}

#[no_mangle]
pub extern "C" fn qi_gui_egui_video_play_impl(id: u64) {
    with_video(id, Video::play);
}

#[no_mangle]
pub extern "C" fn qi_gui_egui_video_pause_impl(id: u64) {
    with_video(id, Video::pause);
}

/// 视频跳到(句柄, 毫秒)：播放中跳过去接着放，暂停中跳过去显示那一帧
#[no_mangle]
pub extern "C" fn qi_gui_egui_video_seek_impl(id: u64, ms: i64) {
    with_video(id, |v| v.seek(ms as f64));
}

#[no_mangle]
pub extern "C" fn qi_gui_egui_video_position_impl(id: u64) -> i64 {
    with_video(id, |v| {
        v.tick();
        v.position().round() as i64
    })
    .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn qi_gui_egui_video_duration_impl(id: u64) -> i64 {
    with_video(id, |v| v.info.duration_ms).unwrap_or(0)
}

/// 视频播放中(句柄) → 1/0；放到头自动停，返回 0
#[no_mangle]
pub extern "C" fn qi_gui_egui_video_playing_impl(id: u64) -> i64 {
    with_video(id, |v| {
        v.tick();
        i64::from(v.playing)
    })
    .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn qi_gui_egui_video_free_impl(id: u64) {
    let v = VIDEOS.with(|vs| vs.borrow_mut().remove(&id));
    drop(v);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::time::Duration;

    /// 用 ffmpeg 现做一段 2 秒的测试视频（彩条 + 正弦音）；没装 ffmpeg 返回 None
    fn make_clip(name: &str, with_audio: bool) -> Option<CString> {
        let ff = video::find_tool("ffmpeg")?;
        let out = std::env::temp_dir().join(format!("qi-gui-test-{name}.mp4"));
        let mut cmd = std::process::Command::new(ff);
        cmd.args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("testsrc=size=320x240:rate=25:duration=2");
        if with_audio {
            cmd.args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2"]);
        }
        cmd.args(["-pix_fmt", "yuv420p", "-shortest"]).arg(&out);
        cmd.status().ok()?.success().then_some(())?;
        CString::new(out.to_string_lossy().as_bytes()).ok()
    }

    #[test]
    fn missing_file_returns_zero() {
        let p = CString::new("/没有/这个/视频.mp4").unwrap();
        assert_eq!(qi_gui_egui_video_load_impl(p.as_ptr()), 0);
        assert_eq!(qi_gui_egui_video_duration_impl(999), 0);
        assert_eq!(qi_gui_egui_video_draw_impl(999, 0), 0);
    }

    #[test]
    fn load_shows_first_frame_then_plays_to_end() {
        let Some(path) = make_clip("silent", false) else {
            eprintln!("没装 ffmpeg，跳过");
            return;
        };
        let id = qi_gui_egui_video_load_impl(path.as_ptr());
        assert!(id > 0);
        assert!((qi_gui_egui_video_duration_impl(id) - 2000).abs() < 100);
        // 不播放也会解出第一帧当封面
        let mut got_first = false;
        for _ in 0..100 {
            with_video(id, |v| v.tick());
            if with_video(id, |v| v.ready.is_some()).unwrap() {
                got_first = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(got_first, "加载后应当解出封面帧");
        assert_eq!(qi_gui_egui_video_position_impl(id), 0);

        // 跳到 1.5 秒播放，应在半秒多后自己停下，位置等于时长
        qi_gui_egui_video_seek_impl(id, 1500);
        qi_gui_egui_video_play_impl(id);
        assert_eq!(qi_gui_egui_video_playing_impl(id), 1);
        let start = Instant::now();
        while qi_gui_egui_video_playing_impl(id) == 1 && start.elapsed() < Duration::from_secs(5) {
            with_video(id, |v| v.ready = None);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(qi_gui_egui_video_playing_impl(id), 0, "放到头应自动停");
        assert_eq!(
            qi_gui_egui_video_position_impl(id),
            qi_gui_egui_video_duration_impl(id)
        );
        assert!(
            start.elapsed() >= Duration::from_millis(350),
            "按墙钟放，不该瞬间放完"
        );
        qi_gui_egui_video_free_impl(id);
        assert_eq!(qi_gui_egui_video_duration_impl(id), 0, "释放后句柄失效");
    }

    /// 拖进度条：一帧一跳，解码器按节流重启，位置却要立刻跟手；松手后停在最后的位置
    #[test]
    fn dragging_seek_bar_is_throttled_but_position_follows() {
        let Some(path) = make_clip("drag", false) else {
            return;
        };
        let id = qi_gui_egui_video_load_impl(path.as_ptr());
        assert!(id > 0);
        qi_gui_egui_video_play_impl(id);
        let start = Instant::now();
        for i in 0..30 {
            qi_gui_egui_video_seek_impl(id, 100 + i * 50);
            assert!((qi_gui_egui_video_position_impl(id) - (100 + i * 50)).abs() < 40);
            std::thread::sleep(Duration::from_millis(16));
        }
        let restarts = with_video(id, |v| v.stats.restarts).unwrap();
        let spent = start.elapsed().as_millis() as u64;
        assert!(
            restarts <= 2 + spent / SEEK_THROTTLE.as_millis() as u64,
            "30 次跳转重启了 {restarts} 次解码器（{spent}ms）"
        );
        // 松手：最后一次跳转一定会落实
        std::thread::sleep(SEEK_THROTTLE);
        qi_gui_egui_video_pause_impl(id);
        with_video(id, |v| v.tick());
        assert!(with_video(id, |v| v.seek_pending.is_none()).unwrap());
        let pos = qi_gui_egui_video_position_impl(id);
        assert!((1550..1800).contains(&pos), "停在 {pos}");
        qi_gui_egui_video_free_impl(id);
    }

    #[test]
    fn frames_have_constant_rate_timestamps_from_seek_point() {
        let Some(path) = make_clip("ts", false) else {
            return;
        };
        let p = video::probe(path.to_str().unwrap()).unwrap();
        assert_eq!((p.width, p.height), (320, 240));
        let dec = VideoDecoder::spawn(path.to_str().unwrap(), 1000.0, 320, 240, p.fps).unwrap();
        let f0 = dec.rx.recv().unwrap();
        let f1 = dec.rx.recv().unwrap();
        assert_eq!(f0.rgba.len(), 320 * 240 * 4);
        assert!((f0.pts_ms - 1000.0).abs() < 0.01);
        assert!((f1.pts_ms - 1040.0).abs() < 0.01, "25fps 一帧 40ms");
        let rest = dec.rx.iter().count();
        assert!(
            (20..=26).contains(&(rest + 2)),
            "从 1 秒起剩约 25 帧，实得 {}",
            rest + 2
        );
    }
}
