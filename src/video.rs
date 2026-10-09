//! 视频解码后端：ffmpeg 子进程 → 原始 RGBA 帧 / PCM 采样
//!
//! 不链接任何编解码库，全靠外部的 `ffmpeg` / `ffprobe` 可执行文件：
//! - 画面：`-vf fps=F,scale=W:H -pix_fmt rgba -f rawvideo` 从管道读，每帧固定
//!   `W*H*4` 字节。`fps` 滤镜把可变帧率规整成恒定帧率，于是第 n 帧的时间戳就是
//!   `起点 + n/F`，不用解析 ffmpeg 的时间戳输出。
//! - 声音：`-f s16le -ac 2 -ar 48000` 从管道读，包成 rodio 的 `Source`。
//!
//! 两路各一个后台线程读管道，经**有界**通道交给主线程：主线程不取，解码线程
//! 就阻塞在发送上，ffmpeg 再阻塞在写管道上 —— 暂停时整条链零 CPU。
//!
//! 没装 ffmpeg 时 `probe` 返回 None，上层据此优雅失败。

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// 解码宽度上限。再宽软光栅也画不动，预览栏也用不着
pub const MAX_DECODE_WIDTH: u32 = 960;
/// 帧率上限：高帧率素材按 30 抽帧
pub const MAX_FPS: f64 = 30.0;
pub const AUDIO_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: u16 = 2;
/// 画面通道容量（帧）。960×540 一帧 2MB，6 帧 12MB
const FRAME_QUEUE: usize = 6;
/// 声音通道容量（块），一块 4096 个采样 ≈ 43ms
const AUDIO_QUEUE: usize = 24;
const AUDIO_CHUNK_BYTES: usize = 4096 * 2;

/// 找 ffmpeg/ffprobe：先 PATH，再几个常见安装位置
/// （从 Finder 启动的 macOS 程序 PATH 里没有 Homebrew）
pub fn find_tool(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        dirs.push(PathBuf::from(extra));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/bin"));
    }
    dirs.into_iter().map(|d| d.join(&exe)).find(|p| p.is_file())
}

fn ffmpeg() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| find_tool("ffmpeg")).as_ref()
}

fn ffprobe() -> Option<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| find_tool("ffprobe")).as_ref()
}

/// ffprobe 量出来的基本信息（宽高已按旋转元数据摆正）
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration_ms: i64,
    pub has_audio: bool,
}

fn parse_rate(s: &str) -> f64 {
    match s.split_once('/') {
        Some((n, d)) => {
            let n: f64 = n.trim().parse().unwrap_or(0.0);
            let d: f64 = d.trim().parse().unwrap_or(0.0);
            if d > 0.0 {
                n / d
            } else {
                0.0
            }
        }
        None => s.trim().parse().unwrap_or(0.0),
    }
}

/// 解析 `ffprobe -of default=noprint_wrappers=1` 的输出。
/// 每条流以 `codec_type=` 打头，后面跟它自己的字段；只认第一条视频流。
pub(crate) fn parse_probe(text: &str) -> Option<Probe> {
    let (mut w, mut h, mut fps, mut avg, mut rot) = (0u32, 0u32, 0.0f64, 0.0f64, 0i64);
    let mut duration = 0.0f64;
    let mut has_audio = false;
    let mut cur = "";
    let mut seen_video = false;
    for line in text.lines() {
        let Some((k, v)) = line.trim().split_once('=') else {
            continue;
        };
        if k == "codec_type" {
            cur = if v == "video" && !seen_video {
                seen_video = true;
                "video"
            } else if v == "audio" {
                has_audio = true;
                "other"
            } else {
                "other"
            };
            continue;
        }
        if k == "duration" {
            if let Ok(d) = v.parse::<f64>() {
                duration = duration.max(d);
            }
            continue;
        }
        if cur != "video" {
            continue;
        }
        match k {
            "width" => w = v.parse().unwrap_or(0),
            "height" => h = v.parse().unwrap_or(0),
            "r_frame_rate" => fps = parse_rate(v),
            "avg_frame_rate" => avg = parse_rate(v),
            "rotation" | "TAG:rotate" => rot = v.parse::<f64>().unwrap_or(0.0) as i64,
            _ => {}
        }
    }
    if w == 0 || h == 0 {
        return None;
    }
    if rot.rem_euclid(180) == 90 {
        std::mem::swap(&mut w, &mut h);
    }
    // avg 是实际平均帧率，r 是"最小公倍数"式的标称值（VFR 会虚高），优先 avg
    let mut rate = if avg > 0.0 { avg } else { fps };
    if !(1.0..=1000.0).contains(&rate) {
        rate = MAX_FPS;
    }
    Some(Probe {
        width: w,
        height: h,
        fps: rate.min(MAX_FPS),
        duration_ms: (duration * 1000.0).round() as i64,
        has_audio,
    })
}

pub fn probe(path: &str) -> Option<Probe> {
    let out = Command::new(ffprobe()?)
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,width,height,r_frame_rate,avg_frame_rate:\
             stream_tags=rotate:stream_side_data=rotation:format=duration",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_probe(&String::from_utf8_lossy(&out.stdout))
}

/// 解码尺寸：宽不超过上限，等比，宽高都取偶数（rawvideo 对奇数尺寸不友好）
pub fn output_size(w: u32, h: u32) -> (u32, u32) {
    let ow = w.clamp(2, MAX_DECODE_WIDTH) & !1;
    let oh = ((ow as f64 * h as f64 / w.max(1) as f64 / 2.0).round() as u32 * 2).max(2);
    (ow, oh)
}

fn spawn_ffmpeg(args: &[String]) -> Option<(Child, ChildStdout)> {
    let mut child = Command::new(ffmpeg()?)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let out = child.stdout.take()?;
    Some((child, out))
}

fn common_args(path: &str, start_ms: f64) -> Vec<String> {
    vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-ss".into(),
        format!("{:.3}", start_ms.max(0.0) / 1000.0),
        "-i".into(),
        path.into(),
        "-sn".into(),
        "-dn".into(),
    ]
}

/// 读满 buf；返回实际读到的字节数（< len 表示到头了）
fn fill(src: &mut impl Read, buf: &mut [u8]) -> usize {
    let mut got = 0;
    while got < buf.len() {
        match src.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    got
}

/// 一帧画面：显示时刻（毫秒，相对文件开头）+ RGBA 像素
pub struct Frame {
    pub pts_ms: f64,
    pub rgba: Vec<u8>,
}

/// 子进程守卫：丢弃时杀掉 ffmpeg 并收尸
pub struct ProcGuard(Child);

impl Drop for ProcGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// 画面解码器：从 `start_ms` 起按恒定帧率吐帧。通道断开 = 解到头了
pub struct VideoDecoder {
    _proc: ProcGuard,
    pub rx: Receiver<Frame>,
}

impl VideoDecoder {
    pub fn spawn(path: &str, start_ms: f64, w: u32, h: u32, fps: f64) -> Option<Self> {
        let mut args = common_args(path, start_ms);
        args.extend(
            [
                "-map",
                "0:v:0",
                "-an",
                "-vf",
                &format!("fps={fps:.6},scale={w}:{h}:flags=bilinear"),
                "-pix_fmt",
                "rgba",
                "-f",
                "rawvideo",
                "pipe:1",
            ]
            .map(String::from),
        );
        let (child, mut out) = spawn_ffmpeg(&args)?;
        let (tx, rx) = sync_channel::<Frame>(FRAME_QUEUE);
        let bytes = (w * h * 4) as usize;
        let step = 1000.0 / fps;
        std::thread::Builder::new()
            .name("qi-video".into())
            .spawn(move || {
                let mut n = 0u64;
                loop {
                    let mut buf = vec![0u8; bytes];
                    if fill(&mut out, &mut buf) < bytes {
                        break;
                    }
                    let pts_ms = start_ms + n as f64 * step;
                    if tx.send(Frame { pts_ms, rgba: buf }).is_err() {
                        break;
                    }
                    n += 1;
                }
            })
            .ok()?;
        Some(VideoDecoder {
            _proc: ProcGuard(child),
            rx,
        })
    }
}

/// 声音解码器的进程端；对应的采样流是 [`PcmSource`]
pub struct AudioFeed {
    _proc: ProcGuard,
}

/// 声音播放进度：已经被音频设备取走的采样帧数 + 是否已经放完
#[derive(Clone, Default)]
pub struct AudioProgress {
    pub frames: Arc<AtomicU64>,
    pub ended: Arc<AtomicBool>,
}

impl AudioProgress {
    pub fn ms(&self) -> f64 {
        self.frames.load(Ordering::Relaxed) as f64 * 1000.0 / AUDIO_RATE as f64
    }
    pub fn is_ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
    }
}

/// 把管道里的 PCM 包成 rodio 的 Source。
///
/// 音画同步以它为钟：rodio 每取走一个采样就记一笔，主线程按"已取走多少"算出
/// 声音放到了哪儿，画面去追它。供不上数时吐静音但**不计数**，钟停住，画面跟着等。
pub struct PcmSource {
    rx: Receiver<Vec<i16>>,
    buf: Vec<i16>,
    pos: usize,
    count: u64,
    pad: u16,
    primed: bool,
    starving: bool,
    done: bool,
    progress: AudioProgress,
}

impl PcmSource {
    fn publish(&self) {
        self.progress
            .frames
            .store(self.count / AUDIO_CHANNELS as u64, Ordering::Relaxed);
    }

    fn refill(&mut self) -> bool {
        // 刚起步等久一点（ffmpeg 要几十毫秒才出第一块）；已经断粮就别再等，
        // 免得音频回调一个采样一个采样地卡
        let wait = if !self.primed {
            Duration::from_millis(300)
        } else if self.starving {
            Duration::ZERO
        } else {
            Duration::from_millis(15)
        };
        let got = if wait.is_zero() {
            self.rx
                .try_recv()
                .map_err(|e| e == TryRecvError::Disconnected)
        } else {
            self.rx
                .recv_timeout(wait)
                .map_err(|e| e == RecvTimeoutError::Disconnected)
        };
        match got {
            Ok(v) => {
                self.buf = v;
                self.pos = 0;
                self.primed = true;
                self.starving = false;
                true
            }
            Err(true) => {
                self.done = true;
                self.publish();
                self.progress.ended.store(true, Ordering::Relaxed);
                false
            }
            Err(false) => {
                self.starving = true;
                false
            }
        }
    }
}

impl Iterator for PcmSource {
    type Item = i16;

    fn next(&mut self) -> Option<i16> {
        if self.pad > 0 {
            self.pad -= 1;
            return Some(0);
        }
        if self.pos >= self.buf.len() && !self.refill() {
            if self.done {
                return None;
            }
            // 静音要整帧地补（左右声道成对），不然恢复供数后声道会错位
            self.pad = AUDIO_CHANNELS - 1;
            return Some(0);
        }
        let s = self.buf[self.pos];
        self.pos += 1;
        self.count += 1;
        if self.count & 0xFF == 0 {
            self.publish();
        }
        Some(s)
    }
}

impl rodio::Source for PcmSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        AUDIO_CHANNELS
    }
    fn sample_rate(&self) -> u32 {
        AUDIO_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// 起一路声音解码：返回进程守卫、交给 rodio 的采样流、进度
pub fn spawn_audio(path: &str, start_ms: f64) -> Option<(AudioFeed, PcmSource, AudioProgress)> {
    let mut args = common_args(path, start_ms);
    args.extend(
        [
            "-map",
            "0:a:0",
            "-vn",
            "-ac",
            &AUDIO_CHANNELS.to_string(),
            "-ar",
            &AUDIO_RATE.to_string(),
            "-f",
            "s16le",
            "pipe:1",
        ]
        .map(String::from),
    );
    let (child, mut out) = spawn_ffmpeg(&args)?;
    let (tx, rx) = sync_channel::<Vec<i16>>(AUDIO_QUEUE);
    std::thread::Builder::new()
        .name("qi-video-audio".into())
        .spawn(move || {
            let mut bytes = vec![0u8; AUDIO_CHUNK_BYTES];
            loop {
                let got = fill(&mut out, &mut bytes);
                let usable = got - got % 4;
                if usable > 0 {
                    let samples: Vec<i16> = bytes[..usable]
                        .chunks_exact(2)
                        .map(|b| i16::from_le_bytes([b[0], b[1]]))
                        .collect();
                    if tx.send(samples).is_err() {
                        break;
                    }
                }
                if got < bytes.len() {
                    break;
                }
            }
        })
        .ok()?;
    let progress = AudioProgress::default();
    let source = PcmSource {
        rx,
        buf: Vec::new(),
        pos: 0,
        count: 0,
        pad: 0,
        primed: false,
        starving: false,
        done: false,
        progress: progress.clone(),
    };
    Some((
        AudioFeed {
            _proc: ProcGuard(child),
        },
        source,
        progress,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_parses_video_and_audio_streams() {
        let text = "codec_type=video\nwidth=1920\nheight=1080\nr_frame_rate=60/1\n\
                    avg_frame_rate=60/1\ncodec_type=audio\nr_frame_rate=0/0\n\
                    avg_frame_rate=0/0\nduration=12.345000\n";
        let p = parse_probe(text).unwrap();
        assert_eq!((p.width, p.height), (1920, 1080));
        assert_eq!(p.fps, MAX_FPS, "60fps 按上限抽到 30");
        assert_eq!(p.duration_ms, 12345);
        assert!(p.has_audio);
    }

    #[test]
    fn probe_swaps_size_for_rotated_phone_video() {
        let text = "codec_type=video\nwidth=1920\nheight=1080\navg_frame_rate=30000/1001\n\
                    rotation=-90\nduration=3.0\n";
        let p = parse_probe(text).unwrap();
        assert_eq!((p.width, p.height), (1080, 1920));
        assert!((p.fps - 29.97).abs() < 0.01);
        assert!(!p.has_audio);
    }

    #[test]
    fn probe_without_video_stream_is_none() {
        assert!(parse_probe("codec_type=audio\nduration=3.0\n").is_none());
    }

    #[test]
    fn output_size_caps_width_and_keeps_even_aspect() {
        assert_eq!(output_size(1920, 1080), (960, 540));
        assert_eq!(output_size(640, 360), (640, 360));
        assert_eq!(output_size(1081, 1921), (960, 1706));
    }

    #[test]
    fn silence_is_padded_in_whole_frames() {
        let (tx, rx) = sync_channel::<Vec<i16>>(4);
        let mut src = PcmSource {
            rx,
            buf: Vec::new(),
            pos: 0,
            count: 0,
            pad: 0,
            primed: true,
            starving: true,
            done: false,
            progress: AudioProgress::default(),
        };
        // 断粮：一整帧（两个声道）静音，且不计入进度
        assert_eq!(src.next(), Some(0));
        tx.send(vec![1, 2, 3, 4]).unwrap();
        assert_eq!(src.next(), Some(0), "补齐右声道");
        assert_eq!(src.next(), Some(1), "恢复后从左声道开始");
        assert_eq!(src.next(), Some(2));
        drop(tx);
        assert_eq!(src.next(), Some(3));
        assert_eq!(src.next(), Some(4));
        assert_eq!(src.next(), None, "通道断开即放完");
        assert!(src.progress.is_ended());
        assert_eq!(src.progress.frames.load(Ordering::Relaxed), 2);
    }
}
