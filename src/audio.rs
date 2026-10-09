/// Audio playback module using rodio
/// Provides simple audio playback capabilities for various formats
///
/// Note: Due to rodio's OutputStream not being Send/Sync, we cannot use
/// a global static. Instead, each audio player manages its own stream.
use rodio::{Decoder, OutputStream, Sink, Source};
use std::fs::File;
use std::io::BufReader;

/// Audio player for a single sound
/// Each player maintains its own audio output stream
pub struct AudioPlayer {
    _stream: OutputStream, // Keep stream alive
    sink: Sink,
    /// 解码器给出的总时长（有的格式给不出，None）
    duration: Option<std::time::Duration>,
}

impl AudioPlayer {
    /// Create a new audio player and load a sound file
    /// Supports: MP3, WAV, FLAC, Vorbis
    pub fn new(file_path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let (stream, stream_handle) = OutputStream::try_default()?;
        let sink = Sink::try_new(&stream_handle)?;

        // Load the audio file
        let file = File::open(file_path)?;
        let source = Decoder::new(BufReader::new(file))?;
        let duration = source.total_duration();

        sink.append(source);
        sink.pause(); // Start paused

        Ok(AudioPlayer {
            _stream: stream,
            sink,
            duration,
        })
    }

    /// Play the audio
    pub fn play(&self) {
        self.sink.play();
    }

    /// Pause the audio
    pub fn pause(&self) {
        self.sink.pause();
    }

    /// Stop the audio
    pub fn stop(&self) {
        self.sink.stop();
    }

    /// Set volume (0.0 to 1.0)
    pub fn set_volume(&self, volume: f32) {
        self.sink.set_volume(volume.max(0.0).min(1.0));
    }

    /// Check if audio is playing
    pub fn is_playing(&self) -> bool {
        !self.sink.is_paused()
    }

    /// Check if audio is finished
    pub fn is_finished(&self) -> bool {
        self.sink.empty()
    }

    /// 已播放到的位置
    pub fn position(&self) -> std::time::Duration {
        self.sink.get_pos()
    }

    /// 总时长（格式不支持时 None）
    pub fn duration(&self) -> Option<std::time::Duration> {
        self.duration
    }

    /// 跳到某个位置；播完了（队列空）就跳不了，返回 false
    pub fn seek(&self, pos: std::time::Duration) -> bool {
        self.sink.try_seek(pos).is_ok()
    }
}

/// Play a sound file once (fire and forget)
/// This is a convenience function for simple sound effects
/// Note: The returned player must be kept alive for the sound to play
pub fn play_sound(file_path: &str) -> Result<AudioPlayer, Box<dyn std::error::Error>> {
    let player = AudioPlayer::new(file_path)?;
    player.play();
    Ok(player)
}

/// Play a sound file in a loop
pub fn play_sound_loop(file_path: &str) -> Result<AudioPlayer, Box<dyn std::error::Error>> {
    let (stream, stream_handle) = OutputStream::try_default()?;
    let sink = Sink::try_new(&stream_handle)?;

    let file = File::open(file_path)?;
    let source = Decoder::new(BufReader::new(file))?;

    // Loop the source
    let source = source.repeat_infinite();
    sink.append(source);
    sink.play();

    Ok(AudioPlayer {
        _stream: stream,
        sink,
        // 循环播放没有总时长
        duration: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // 要一台真的音频输出设备：CI runner（Linux 无声卡、Windows 无音频端点）上
    // 必挂。本机跑 cargo test -- --ignored 照样能测。
    #[test]
    #[ignore = "需要真实音频输出设备，CI runner 没有"]
    fn test_audio_player_creation() {
        // Test that we can create an audio player structure
        // (cannot test actual playback without audio files)
        let result = OutputStream::try_default();
        assert!(result.is_ok());
    }
}
