//! Audio player example
//!
//! Demonstrates basic audio playback functionality.
//! Note: You need to provide your own audio files to test this.

use qi_gui::audio::AudioPlayer;
use std::thread;
use std::time::Duration;

fn main() {
    println!("=== Audio Player Example ===");
    println!();

    // Check if audio file is provided
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        println!("Usage: {} <audio_file.mp3|wav|flac>", args[0]);
        println!();
        println!("Example:");
        println!("  {} music.mp3", args[0]);
        println!();
        println!("Supported formats: MP3, WAV, FLAC, Vorbis");
        return;
    }

    let audio_file = &args[1];
    println!("Loading audio file: {}", audio_file);

    // Load audio
    let player = match AudioPlayer::new(audio_file) {
        Ok(p) => {
            println!("Audio loaded successfully!");
            p
        }
        Err(e) => {
            eprintln!("Failed to load audio file: {}", e);
            return;
        }
    };

    println!();
    println!("Controls:");
    println!("  Starting playback...");

    // Play audio
    player.play();
    println!("  ▶ Playing");

    // Wait a bit
    thread::sleep(Duration::from_secs(3));

    // Pause
    println!("  ⏸ Pausing");
    player.pause();
    thread::sleep(Duration::from_secs(2));

    // Resume
    println!("  ▶ Resuming");
    player.play();
    thread::sleep(Duration::from_secs(2));

    // Adjust volume
    println!("  🔉 Setting volume to 50%");
    player.set_volume(0.5);
    thread::sleep(Duration::from_secs(2));

    println!("  🔊 Setting volume to 100%");
    player.set_volume(1.0);
    thread::sleep(Duration::from_secs(2));

    // Stop
    println!("  ⏹ Stopping");
    player.stop();

    println!();
    println!("Demo completed!");
}
