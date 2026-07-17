use gpui::{App, AppContext, Entity, Global};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
}

pub type MicCaptureHandle = Box<dyn Send>;

pub type MicCaptureFactory =
    Arc<dyn Fn(&str, flume::Sender<f32>) -> Result<MicCaptureHandle, String> + Send + Sync>;

#[derive(Debug, Clone, Copy)]
pub struct MicPcmFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

pub type MicPcmCaptureFactory = Arc<
    dyn Fn(flume::Sender<Vec<f32>>) -> Result<(MicCaptureHandle, MicPcmFormat), String>
        + Send
        + Sync,
>;

pub type DeviceEnumerator =
    Arc<dyn Fn() -> (Vec<AudioDeviceInfo>, Vec<AudioDeviceInfo>) + Send + Sync>;

pub struct AudioStore {
    pub input_devices: Vec<AudioDeviceInfo>,
    pub output_devices: Vec<AudioDeviceInfo>,
    pub mic_capture_factory: Option<MicCaptureFactory>,
    pub mic_pcm_capture_factory: Option<MicPcmCaptureFactory>,
    device_enumerator: Option<DeviceEnumerator>,
    devices_requested: bool,
}

impl AudioStore {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            input_devices: Vec::new(),
            output_devices: Vec::new(),
            mic_capture_factory: None,
            mic_pcm_capture_factory: None,
            device_enumerator: None,
            devices_requested: false,
        });
        cx.set_global(GlobalAudioStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalAudioStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalAudioStore>().map(|g| g.0.clone())
    }

    pub fn set_devices(
        entity: &Entity<Self>,
        input: Vec<AudioDeviceInfo>,
        output: Vec<AudioDeviceInfo>,
        cx: &mut App,
    ) {
        entity.update(cx, |store, cx| {
            store.input_devices = input;
            store.output_devices = output;
            cx.notify();
        });
    }

    pub fn set_mic_capture_factory(
        entity: &Entity<Self>,
        factory: MicCaptureFactory,
        cx: &mut App,
    ) {
        entity.update(cx, |store, cx| {
            store.mic_capture_factory = Some(factory);
            cx.notify();
        });
    }

    pub fn set_device_enumerator(
        entity: &Entity<Self>,
        enumerator: DeviceEnumerator,
        cx: &mut App,
    ) {
        entity.update(cx, |store, _| {
            store.device_enumerator = Some(enumerator);
        });
    }

    pub fn ensure_devices(entity: &Entity<Self>, cx: &mut App) {
        let store = entity.read(cx);
        if store.devices_requested {
            return;
        }
        let Some(enumerator) = store.device_enumerator.clone() else {
            return;
        };
        entity.update(cx, |store, _| {
            store.devices_requested = true;
        });
        let weak = entity.downgrade();
        cx.spawn(async move |cx: &mut gpui::AsyncApp| {
            let (inputs, outputs) = cx
                .background_executor()
                .spawn(async move { enumerator() })
                .await;
            cx.update(|cx| {
                if let Some(store) = weak.upgrade() {
                    AudioStore::set_devices(&store, inputs, outputs, cx);
                }
            });
        })
        .detach();
    }

    pub fn set_mic_pcm_capture_factory(
        entity: &Entity<Self>,
        factory: MicPcmCaptureFactory,
        cx: &mut App,
    ) {
        entity.update(cx, |store, _| {
            store.mic_pcm_capture_factory = Some(factory);
        });
    }

    pub fn start_mic_capture(
        &self,
        device_id: &str,
        sender: flume::Sender<f32>,
    ) -> Result<MicCaptureHandle, String> {
        match &self.mic_capture_factory {
            Some(factory) => factory(device_id, sender),
            None => Err("Mic capture not available on this platform".to_string()),
        }
    }

    pub fn start_mic_pcm_capture(
        &self,
        sender: flume::Sender<Vec<f32>>,
    ) -> Result<(MicCaptureHandle, MicPcmFormat), String> {
        match &self.mic_pcm_capture_factory {
            Some(factory) => factory(sender),
            None => Err("Mic capture not available on this platform".to_string()),
        }
    }
}

impl gpui::EventEmitter<()> for AudioStore {}

struct GlobalAudioStore(Entity<AudioStore>);
impl Global for GlobalAudioStore {}
