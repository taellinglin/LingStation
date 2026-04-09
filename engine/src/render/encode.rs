use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::audio::{AudioClipCache, TrackAudioState};
use crate::render::render_plan_for_each_block;
use crate::render::RenderPlan;

fn f32_to_i16_sample(s: f32) -> i16 {
    let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i32;
    v.clamp(-32768, 32767) as i16
}

fn f32_to_i32_sample(s: f32) -> i32 {
    (s.clamp(-1.0, 1.0) * 32767.0).round() as i32
}

/// Offline FLAC export via flacenc (`Vec<i32>` buffer after the offline mix pass).
pub fn offline_render_plan_to_flac(
    plan: &RenderPlan,
    out_path: &Path,
    progress_done: &AtomicU64,
    progress_total: &AtomicU64,
    track_audio: Vec<TrackAudioState>,
    audio_clip_cache: &Arc<Mutex<AudioClipCache>>,
) -> Result<(), String> {
    use flacenc::bitsink::ByteSink;
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;

    let sample_rate = plan.sample_rate.max(1) as usize;
    let channels = plan.channels as usize;
    let ch = channels.max(1);
    let bpm = plan.bpm;
    let samples_per_beat = (plan.sample_rate as f64 * 60.0) / bpm as f64;
    let start_samples = (plan.start_beats as f64 * samples_per_beat).round() as u64;
    let end_samples = (plan.end_beats as f64 * samples_per_beat).round() as u64;
    let expected_total = end_samples.saturating_sub(start_samples);
    progress_total.store(expected_total.max(1), Ordering::Relaxed);
    progress_done.store(0, Ordering::Relaxed);

    let mut all_samples: Vec<i32> = Vec::new();
    let cancel = AtomicBool::new(false);

    render_plan_for_each_block(
        plan,
        &cancel,
        expected_total,
        track_audio,
        audio_clip_cache,
        |block, _frames| {
            for &s in block {
                all_samples.push(f32_to_i32_sample(s));
            }
            let frames_done = (all_samples.len() / ch) as u64;
            progress_done.store(frames_done.min(expected_total), Ordering::Relaxed);
            Ok(())
        },
    )?;

    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_, e)| format!("FLAC config: {e:?}"))?;
    let source = flacenc::source::MemSource::from_samples(&all_samples, ch, 16, sample_rate);
    let flac_stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| format!("FLAC encode: {e}"))?;
    let mut sink = ByteSink::new();
    BitRepr::write(&flac_stream, &mut sink).map_err(|e| format!("FLAC serialize: {e}"))?;
    std::fs::write(out_path, sink.as_slice()).map_err(|e| e.to_string())?;
    progress_done.store(expected_total.max(1), Ordering::Relaxed);
    Ok(())
}

/// Offline Ogg Vorbis export via `vorbis-encoder` (needs libvorbis/libogg when linking).
pub fn offline_render_plan_to_ogg_vorbis(
    plan: &RenderPlan,
    out_path: &Path,
    progress_done: &AtomicU64,
    progress_total: &AtomicU64,
    track_audio: Vec<TrackAudioState>,
    audio_clip_cache: &Arc<Mutex<AudioClipCache>>,
    quality: f32,
) -> Result<(), String> {
    let sample_rate = plan.sample_rate;
    let channels = plan.channels.max(1) as u32;

    let mut enc = vorbis_encoder::Encoder::new(channels, sample_rate as u64, quality)
        .map_err(|e| format!("Vorbis encoder init failed (is libvorbis installed?): code {e}"))?;

    let mut file = File::create(out_path).map_err(|e| e.to_string())?;
    let empty: Vec<i16> = Vec::new();
    let headers = enc
        .encode(&empty)
        .map_err(|e| format!("Vorbis header packet: {e}"))?;
    file.write_all(&headers).map_err(|e| e.to_string())?;

    let cancel = AtomicBool::new(false);
    let bpm = plan.bpm;
    let samples_per_beat = (sample_rate as f64 * 60.0) / bpm as f64;
    let start_samples = (plan.start_beats as f64 * samples_per_beat).round() as u64;
    let end_samples = (plan.end_beats as f64 * samples_per_beat).round() as u64;
    let expected_total = end_samples.saturating_sub(start_samples);
    progress_total.store(expected_total.max(1), Ordering::Relaxed);
    progress_done.store(0, Ordering::Relaxed);

    let mut rendered = 0u64;
    render_plan_for_each_block(
        plan,
        &cancel,
        expected_total,
        track_audio,
        audio_clip_cache,
        |block, frames| {
            let mut pcm: Vec<i16> = Vec::with_capacity(block.len());
            for &s in block {
                pcm.push(f32_to_i16_sample(s));
            }
            let chunk = enc
                .encode(&pcm)
                .map_err(|e| format!("Vorbis encode: {e}"))?;
            file.write_all(&chunk).map_err(|e| e.to_string())?;
            rendered = rendered.saturating_add(frames as u64);
            progress_done.store(rendered.min(expected_total), Ordering::Relaxed);
            Ok(())
        },
    )?;

    let tail = enc.flush().map_err(|e| format!("Vorbis flush: {e}"))?;
    file.write_all(&tail).map_err(|e| e.to_string())?;
    progress_done.store(expected_total.max(1), Ordering::Relaxed);
    Ok(())
}
