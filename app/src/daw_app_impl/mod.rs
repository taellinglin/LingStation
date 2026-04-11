use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use engine::audio::{AudioClipCache, TrackAudioState, MAX_PLUGIN_OUTPUT_CHANNELS};
use engine::hosts::clap as clap_host;
use engine::hosts::vst3;
use engine::midi::{export_midi, export_midi_multitrack, import_midi_channels, import_midi_tracks};
use engine::models::*;
use engine::performance::performance_length_samples;
use engine::performance::{PerformanceRuntimeClip, PerformanceTriggerMode};
use engine::render::*;
use crate::daw_app::AiScoreJobResult;
use parking_lot::Mutex as ParkingMutex;

use super::*;

include!("eframe_app.rs");
include!("drop.rs");
include!("impl_sync_undo.rs");
include!("impl_clip_editing.rs");
include!("impl_performance.rs");
include!("impl_theme_params.rs");
include!("impl_audio_waveform.rs");
include!("impl_chrome.rs");
include!("impl_sidebar_fs.rs");
include!("impl_panels.rs");
include!("impl_center_arranger.rs");
include!("impl_center_views.rs");
include!("impl_center_performance.rs");
include!("impl_params_roll.rs");
include!("impl_modals.rs");
include!("impl_midi_io.rs");
include!("impl_render_plan.rs");
include!("impl_treesynth_presets.rs");
include!("impl_drummachine.rs");
include!("impl_ai_scores.rs");
include!("impl_project_tail.rs");
