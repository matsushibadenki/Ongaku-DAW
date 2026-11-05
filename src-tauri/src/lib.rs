#![allow(clippy::needless_pass_by_value)]

use audio_backend_cpal::CpalAudioBackend;
use audio_types::{AudioConfig, AudioDeviceInfo, RealtimeMetricsSnapshot};
use daw_app::DawApplication;
use daw_core::{
    SamplePosition, SampleRange,
    transport::{TransportCommand, TransportSnapshot},
};
use daw_project::ProjectSnapshot;
use std::sync::Mutex;
use tauri::State;

struct ApplicationState {
    application: Mutex<DawApplication>,
    backend: CpalAudioBackend,
}

#[tauri::command]
fn get_document_status(
    state: State<'_, ApplicationState>,
) -> Result<daw_app::DocumentStatus, String> {
    Ok(state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned")?
        .document_status())
}

#[tauri::command]
fn undo_project(state: State<'_, ApplicationState>) -> Result<ProjectSnapshot, String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned")?
        .undo()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn redo_project(state: State<'_, ApplicationState>) -> Result<ProjectSnapshot, String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned")?
        .redo()
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_project(
    save_as: bool,
    app: tauri::AppHandle,
    state: State<'_, ApplicationState>,
) -> Result<Option<daw_app::DocumentStatus>, String> {
    use tauri_plugin_dialog::DialogExt;
    let current = state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned")?
        .document_status()
        .path;
    let path = if !save_as && current.is_some() {
        current.map(std::path::PathBuf::from)
    } else {
        app.dialog()
            .file()
            .add_filter("Ongaku", &["ongaku"])
            .set_file_name("Session.ongaku")
            .blocking_save_file()
            .map(|p| p.into_path().map_err(|e| e.to_string()))
            .transpose()?
    };
    let Some(path) = path else {
        return Ok(None);
    };
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned")?
        .save_project(&path)
        .map(Some)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn load_project(
    app: tauri::AppHandle,
    state: State<'_, ApplicationState>,
) -> Result<Option<ProjectSnapshot>, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(file) = app
        .dialog()
        .file()
        .add_filter("Ongaku", &["ongaku"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = file.into_path().map_err(|e| e.to_string())?;
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned")?
        .load_project(&path)
        .map(Some)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_output_devices(state: State<'_, ApplicationState>) -> Result<Vec<AudioDeviceInfo>, String> {
    DawApplication::output_devices(&state.backend).map_err(|error| error.to_string())
}

#[tauri::command]
fn default_output_config(state: State<'_, ApplicationState>) -> Result<AudioConfig, String> {
    DawApplication::default_config(&state.backend).map_err(|error| error.to_string())
}

#[tauri::command]
fn start_engine(
    device_id: Option<String>,
    config: AudioConfig,
    state: State<'_, ApplicationState>,
) -> Result<(), String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .start(&state.backend, device_id.as_deref(), config)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn stop_engine(state: State<'_, ApplicationState>) -> Result<(), String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .stop()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_gain(gain: f32, state: State<'_, ApplicationState>) -> Result<(), String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .set_gain(gain)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_metrics(
    state: State<'_, ApplicationState>,
) -> Result<Option<RealtimeMetricsSnapshot>, String> {
    Ok(state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .metrics())
}

#[tauri::command]
fn configure_project(
    sample_rate: u32,
    state: State<'_, ApplicationState>,
) -> Result<ProjectSnapshot, String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .configure_project(sample_rate)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_project_snapshot(state: State<'_, ApplicationState>) -> Result<ProjectSnapshot, String> {
    Ok(state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .project())
}

#[tauri::command]
fn get_transport_snapshot(
    state: State<'_, ApplicationState>,
) -> Result<Option<TransportSnapshot>, String> {
    let application = state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?;
    Ok(application.transport().ok())
}

#[tauri::command]
fn play_transport(state: State<'_, ApplicationState>) -> Result<(), String> {
    send_transport(&state, TransportCommand::Play)
}

#[tauri::command]
fn pause_transport(state: State<'_, ApplicationState>) -> Result<(), String> {
    send_transport(&state, TransportCommand::Stop)
}

#[tauri::command]
fn seek_transport(sample: u64, state: State<'_, ApplicationState>) -> Result<(), String> {
    send_transport(&state, TransportCommand::Seek(SamplePosition(sample)))
}

#[tauri::command]
fn set_transport_loop(
    enabled: bool,
    start_sample: u64,
    end_sample: u64,
    state: State<'_, ApplicationState>,
) -> Result<(), String> {
    let range = enabled
        .then(|| SampleRange::new(SamplePosition(start_sample), SamplePosition(end_sample)))
        .flatten();
    if enabled && range.is_none() {
        return Err("loop end must be after loop start".into());
    }
    send_transport(&state, TransportCommand::SetLoop(range))
}

#[tauri::command]
fn set_track_gain(
    track_id: u64,
    gain: f32,
    state: State<'_, ApplicationState>,
) -> Result<(), String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .set_track_gain(track_id, gain)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_track_pan(
    track_id: u64,
    pan: f32,
    state: State<'_, ApplicationState>,
) -> Result<(), String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .set_track_pan(track_id, pan)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_track_mute(
    track_id: u64,
    muted: bool,
    state: State<'_, ApplicationState>,
) -> Result<(), String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .set_track_mute(track_id, muted)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_track_solo(
    track_id: u64,
    solo: bool,
    state: State<'_, ApplicationState>,
) -> Result<(), String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .set_track_solo(track_id, solo)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_graph_publication_status(
    after_revision: u64,
    state: State<'_, ApplicationState>,
) -> Result<daw_app::PublicationStatus, String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .service_publications(after_revision)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_mixer_event(
    track_id: u64,
    event: daw_project::MixerEvent,
    state: State<'_, ApplicationState>,
) -> Result<ProjectSnapshot, String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .set_mixer_event(track_id, event)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn remove_mixer_event(
    track_id: u64,
    event_id: u64,
    state: State<'_, ApplicationState>,
) -> Result<ProjectSnapshot, String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .remove_mixer_event(track_id, event_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn reorder_tracks(
    track_ids: Vec<u64>,
    state: State<'_, ApplicationState>,
) -> Result<ProjectSnapshot, String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .reorder_tracks(&track_ids)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn edit_midi_clip(
    track_id: u64,
    clip_id: u64,
    action: String,
    amount: i32,
    note_id: Option<u64>,
    state: State<'_, ApplicationState>,
) -> Result<ProjectSnapshot, String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .edit_midi_clip(track_id, clip_id, &action, amount, note_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn edit_midi_clips(
    targets: Vec<daw_project::MidiClipTarget>,
    edit: daw_project::MidiClipEdit,
    state: State<'_, ApplicationState>,
) -> Result<ProjectSnapshot, String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned")?
        .edit_midi_clips(&targets, edit)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn edit_midi_notes(
    targets: Vec<daw_project::MidiNoteTarget>,
    edit: daw_project::MidiSelectionEdit,
    state: State<'_, ApplicationState>,
) -> Result<ProjectSnapshot, String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned")?
        .edit_midi_notes(&targets, edit)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn add_midi_clip(
    track_id: u64,
    start: u64,
    duration_samples: u64,
    state: State<'_, ApplicationState>,
) -> Result<ProjectSnapshot, String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned")?
        .add_midi_clip(track_id, start, duration_samples)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn edit_midi_selection(
    track_id: u64,
    clip_id: u64,
    note_ids: Vec<u64>,
    edit: daw_project::MidiSelectionEdit,
    state: State<'_, ApplicationState>,
) -> Result<ProjectSnapshot, String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned")?
        .edit_midi_selection(track_id, clip_id, &note_ids, edit)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn write_midi_note(
    track_id: u64,
    clip_id: u64,
    note_id: Option<u64>,
    start: u64,
    duration_samples: u64,
    key: u8,
    state: State<'_, ApplicationState>,
) -> Result<ProjectSnapshot, String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned")?
        .write_midi_note(track_id, clip_id, note_id, start, duration_samples, key)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_midi_note_velocity(
    track_id: u64,
    clip_id: u64,
    note_id: u64,
    velocity: f32,
    state: State<'_, ApplicationState>,
) -> Result<ProjectSnapshot, String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .set_midi_note_velocity(track_id, clip_id, note_id, velocity)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_automation_point_value(
    track_id: u64,
    lane_id: u64,
    point_id: u64,
    value: f32,
    state: State<'_, ApplicationState>,
) -> Result<ProjectSnapshot, String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .set_automation_point_value(track_id, lane_id, point_id, value)
        .map_err(|error| error.to_string())
}

fn send_transport(
    state: &State<'_, ApplicationState>,
    command: TransportCommand,
) -> Result<(), String> {
    state
        .application
        .lock()
        .map_err(|_| "application state lock was poisoned".to_owned())?
        .send_transport(command)
        .map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Starts the Tauri application shell.
///
/// # Panics
///
/// Panics when the native application runtime cannot be initialized.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(ApplicationState {
            application: Mutex::new(DawApplication::new_with_background_preparation()),
            backend: CpalAudioBackend,
        })
        .invoke_handler(tauri::generate_handler![
            get_document_status,
            undo_project,
            redo_project,
            save_project,
            load_project,
            list_output_devices,
            default_output_config,
            start_engine,
            stop_engine,
            set_gain,
            get_metrics,
            configure_project,
            get_project_snapshot,
            get_transport_snapshot,
            play_transport,
            pause_transport,
            seek_transport,
            set_transport_loop,
            set_track_gain,
            set_track_pan,
            set_track_mute,
            set_track_solo,
            reorder_tracks,
            set_mixer_event,
            get_graph_publication_status,
            remove_mixer_event,
            edit_midi_clip,
            edit_midi_clips,
            edit_midi_notes,
            add_midi_clip,
            edit_midi_selection,
            write_midi_note,
            set_midi_note_velocity,
            set_automation_point_value,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Ongaku DAW application");
}
