use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use aura_gemini::session::GeminiLiveSession;
use aura_memory::SessionMemory;
use aura_menubar::app::MenuBarMessage;
use aura_voice::playback::AudioPlayer;

use crate::bus::EventBus;
use crate::protocol::DaemonEvent;

/// Cloud service configuration.
#[derive(Clone, Default)]
pub struct CloudConfig {
    pub gemini_api_key: String,
    pub cloud_run_url: Option<String>,
    pub device_token: Option<String>,
    pub cloud_run_device_id: Option<String>,
    pub firestore_project_id: Option<String>,
    pub firebase_api_key: Option<String>,
}

/// Shared atomic flags for cross-task coordination.
#[derive(Clone)]
pub struct SharedFlags {
    pub is_speaking: Arc<AtomicBool>,
    pub is_interrupted: Arc<AtomicBool>,
    pub has_permission_error: Arc<AtomicBool>,
}

impl SharedFlags {
    pub fn new() -> Self {
        Self {
            is_speaking: Arc::new(AtomicBool::new(false)),
            is_interrupted: Arc::new(AtomicBool::new(false)),
            has_permission_error: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Default for SharedFlags {
    fn default() -> Self {
        Self::new()
    }
}

/// Central context shared across all daemon subsystems.
pub struct DaemonContext {
    pub session: Arc<GeminiLiveSession>,
    pub bus: EventBus,
    pub cancel: CancellationToken,
    pub memory: Arc<Mutex<SessionMemory>>,
    pub session_id: String,
    pub menubar_tx: Option<mpsc::Sender<MenuBarMessage>>,
    pub ipc_tx: broadcast::Sender<DaemonEvent>,
    pub player: Option<AudioPlayer>,
    pub cloud: CloudConfig,
    pub flags: SharedFlags,
}
