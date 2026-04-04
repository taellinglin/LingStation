use crate::audio::{AudioClipCache, AudioRuntimeBuffers, TrackAudioState};
use crate::hosts::vst3::{self};
use crate::models::*;
use crate::node_editor::TrackNodeActivity;
use parking_lot::Mutex;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

pub mod mix;
pub mod util;

pub use mix::*;
pub use util::*;

thread_local! {
    pub static MIX_TEMP: RefCell<Vec<f32>> = RefCell::new(Vec::with_capacity(2048));
    pub static FX_TEMP: RefCell<Vec<f32>> = RefCell::new(Vec::with_capacity(2048));
    pub static HOST_OUTPUT_TEMP: RefCell<Vec<f32>> = RefCell::new(Vec::with_capacity(2048));
    pub static EVENTS_TMP: RefCell<Vec<vst3::MidiEvent>> = RefCell::new(Vec::with_capacity(512));
    pub static FILTERED_EVENTS_TMP: RefCell<Vec<vst3::MidiEvent>> = RefCell::new(Vec::with_capacity(512));
    pub static PERF_EVENTS_TMP: RefCell<Vec<vst3::MidiEvent>> = RefCell::new(Vec::with_capacity(512));
    pub static REMAINING_PARAMS_TMP: RefCell<Vec<PendingParamChange>> = RefCell::new(Vec::with_capacity(128));
}

pub fn wav_spec_for_depth(
    sample_rate: u32,
    channels: u16,
    bit_depth: RenderWavBitDepth,
) -> hound::WavSpec {
    hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: bit_depth.bits_per_sample(),
        sample_format: bit_depth.sample_format(),
    }
}

pub fn sample_to_int(sample: f32, bits: u16) -> i32 {
    let max = (1i64 << (bits.saturating_sub(1))) - 1;
    let min = -(1i64 << (bits.saturating_sub(1)));
    let scaled = (sample.clamp(-1.0, 1.0) * max as f32).round() as i64;
    scaled.clamp(min, max) as i32
}

pub fn write_wav_samples<W: std::io::Write + std::io::Seek>(
    writer: &mut hound::WavWriter<W>,
    bit_depth: RenderWavBitDepth,
    samples: &[f32],
) -> Result<(), String> {
    match bit_depth {
        RenderWavBitDepth::Float32 => {
            for sample in samples {
                writer.write_sample(*sample).map_err(|e| e.to_string())?;
            }
        }
        RenderWavBitDepth::Int16 => {
            for sample in samples {
                let value = sample_to_int(*sample, 16) as i16;
                writer.write_sample(value).map_err(|e| e.to_string())?;
            }
        }
        RenderWavBitDepth::Int24 => {
            for sample in samples {
                let value = sample_to_int(*sample, 24);
                writer.write_sample(value).map_err(|e| e.to_string())?;
            }
        }
        RenderWavBitDepth::Int32 => {
            for sample in samples {
                let value = sample_to_int(*sample, 32);
                writer.write_sample(value).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct RenderPlan {
    pub start_beats: f32,
    pub end_beats: f32,
    pub sample_rate: u32,
    pub channels: u16,
    pub bit_depth: RenderWavBitDepth,
    pub bpm: f32,
    pub tracks: Vec<RenderTrack>,
    pub master_comp: MasterCompSettings,
    pub license_comment: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RenderTrack {
    pub index: usize,
    pub mix: TrackMixState,
    pub treesynth_enabled: bool,
    pub treesynth_state: Option<TreeSynthState>,
    pub host_id: Option<String>,
    pub host_state: Option<Vec<u8>>,
    pub clips: Vec<Clip>,
    pub automation: Vec<AutomationLane>,
}

pub fn render_plan_for_each_block<F>(
    plan: &RenderPlan,
    done: &AtomicBool,
    _total_samples_hint: u64,
    track_audio: Vec<TrackAudioState>,
    audio_clip_cache: &Arc<Mutex<AudioClipCache>>,
    mut callback: F,
) -> Result<u64, String>
where
    F: FnMut(&[f32], usize) -> Result<(), String>,
{
    let sample_rate = plan.sample_rate;
    let channels = plan.channels as usize;
    let bpm = plan.bpm;
    let samples_per_beat = (sample_rate as f64 * 60.0) / bpm as f64;
    let start_samples = (plan.start_beats as f64 * samples_per_beat).round() as u64;
    let end_samples = (plan.end_beats as f64 * samples_per_beat).round() as u64;
    let total_samples = end_samples.saturating_sub(start_samples);
    if total_samples == 0 {
        return Ok(0);
    }

    let block_size = 1024;
    let mut output = vec![0.0f32; block_size * channels];
    let mut runtime_buffers = AudioRuntimeBuffers::new(track_audio.len(), block_size);

    let tempo_bits = AtomicU32::new(bpm.to_bits());
    let transport_samples = AtomicU64::new(start_samples);
    let loop_start_samples = AtomicU64::new(0);
    let loop_end_samples = AtomicU64::new(0);
    let playback_panic = AtomicBool::new(false);
    let arrangement_playback_enabled = AtomicBool::new(true);

    let mut current_samples = start_samples;
    while current_samples < end_samples && !done.load(Ordering::Relaxed) {
        let remaining = end_samples - current_samples;
        let frames = (remaining as usize).min(block_size);
        let block_output = &mut output[..frames * channels];
        block_output.fill(0.0);

        let mix_snap = Arc::new(Mutex::new(
            plan.tracks
                .iter()
                .map(|t| t.mix.clone())
                .collect::<Vec<_>>(),
        ));
        let node_activity = Arc::new(Mutex::new(vec![
            TrackNodeActivity::default();
            plan.tracks.len()
        ]));
        let node_routes = Arc::new(Mutex::new(Vec::new()));
        let perf_runtime = Arc::new(Mutex::new(vec![None; plan.tracks.len()]));
        let audio_clips = Arc::new(Mutex::new(
            plan.tracks
                .iter()
                .flat_map(|t| {
                    t.clips.iter().map(|c| {
                        let mut render = AudioClipRender::from_clip(c, t.index);
                        render.path = c.audio_path.clone().unwrap_or_default();
                        render
                    })
                })
                .collect::<Vec<_>>(),
        ));

        mix_track_hosts(
            block_output,
            channels,
            sample_rate as f32,
            &tempo_bits,
            &transport_samples,
            &loop_start_samples,
            &loop_end_samples,
            &playback_panic,
            &arrangement_playback_enabled,
            &track_audio,
            &mix_snap,
            &node_activity,
            &node_routes,
            &perf_runtime,
            &audio_clips,
            audio_clip_cache,
            false,
            false,
            &mut runtime_buffers,
        );

        callback(block_output, frames)?;
        current_samples += frames as u64;
    }

    Ok(total_samples)
}

pub fn build_wav_info_chunk(comment: &str) -> Vec<u8> {
    let mut info = Vec::new();
    info.extend_from_slice(b"INFO");
    info.extend_from_slice(b"ICMT");
    let mut text = comment.as_bytes().to_vec();
    text.push(0);
    let text_len = text.len() as u32;
    info.extend_from_slice(&text_len.to_le_bytes());
    info.extend_from_slice(&text);
    if text_len % 2 == 1 {
        info.push(0);
    }
    let list_size = info.len() as u32;
    let mut chunk = Vec::new();
    chunk.extend_from_slice(b"LIST");
    chunk.extend_from_slice(&list_size.to_le_bytes());
    chunk.extend_from_slice(&info);
    chunk
}

pub fn append_wav_comment(path: &str, comment: &str) -> Result<(), String> {
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let file_len = file.metadata().map_err(|e| e.to_string())?.len();
    if file_len < 12 {
        return Err("Invalid WAV file".to_string());
    }
    let chunk = build_wav_info_chunk(comment);
    let new_len = file_len.saturating_add(chunk.len() as u64);
    if new_len > u32::MAX as u64 + 8 {
        return Err("WAV too large for metadata".to_string());
    }
    let riff_size = (new_len - 8) as u32;
    file.seek(SeekFrom::Start(4)).map_err(|e| e.to_string())?;
    file.write_all(&riff_size.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    file.write_all(&chunk).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn build_flac_comment_block(comment: &str) -> Vec<u8> {
    let vendor = "LingStation";
    let mut payload = Vec::new();
    payload.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    payload.extend_from_slice(vendor.as_bytes());
    let comment_entry = format!("COMMENT={}", comment);
    payload.extend_from_slice(&1u32.to_le_bytes());
    payload.extend_from_slice(&(comment_entry.len() as u32).to_le_bytes());
    payload.extend_from_slice(comment_entry.as_bytes());

    let len = payload.len();
    let mut block = Vec::with_capacity(4 + len);
    block.push(0x84);
    block.push(((len >> 16) & 0xff) as u8);
    block.push(((len >> 8) & 0xff) as u8);
    block.push((len & 0xff) as u8);
    block.extend_from_slice(&payload);
    block
}
