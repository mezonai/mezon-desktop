use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use mezon_voice::{MediaDevice, MediaPermission};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaPermissionPrompt {
    Request {
        device: MediaDevice,
        requesting: bool,
    },
    Blocked(MediaDevice),
}

impl MediaPermissionPrompt {
    pub fn device(self) -> MediaDevice {
        match self {
            Self::Request { device, .. } | Self::Blocked(device) => device,
        }
    }
}

type GrantedAction = Box<dyn FnOnce(&mut App)>;

struct GlobalMediaPermissionStore(Entity<MediaPermissionStore>);
impl Global for GlobalMediaPermissionStore {}

pub struct MediaPermissionStore {
    microphone: MediaPermission,
    camera: MediaPermission,
    prompt: Option<MediaPermissionPrompt>,
    on_granted: Option<GrantedAction>,
    denial_blocks: bool,
    epoch: u64,
    _changes: Task<()>,
    _refresh: Option<Task<()>>,
}

impl MediaPermissionStore {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(Self::new);
        cx.set_global(GlobalMediaPermissionStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalMediaPermissionStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalMediaPermissionStore>()
            .map(|g| g.0.clone())
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let changes = mezon_voice::media_permission_changes();
        let changes_task = cx.spawn(async move |this, cx| {
            while let Ok(device) = changes.recv_async().await {
                let status = cx
                    .background_spawn(async move { mezon_voice::media_permission(device) })
                    .await;
                if this
                    .update(cx, |this, cx| this.apply_live(device, status, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut this = Self {
            microphone: MediaPermission::Granted,
            camera: MediaPermission::Granted,
            prompt: None,
            on_granted: None,
            denial_blocks: mezon_voice::MEDIA_DENIAL_IS_AUTHORITATIVE,
            epoch: 0,
            _changes: changes_task,
            _refresh: None,
        };
        this.refresh(cx);
        this
    }

    pub fn ensure_global(
        device: MediaDevice,
        on_granted: impl FnOnce(&mut App) + 'static,
        cx: &mut App,
    ) -> bool {
        match Self::try_global(cx) {
            Some(store) => store.update(cx, |store, cx| store.ensure(device, on_granted, cx)),
            None => true,
        }
    }

    pub fn warn_if_denied_global(device: MediaDevice, cx: &mut App) -> bool {
        Self::try_global(cx)
            .is_some_and(|store| store.update(cx, |store, cx| store.warn_if_denied(device, cx)))
    }

    pub fn blocked_global(device: MediaDevice, cx: &mut App) -> bool {
        Self::try_global(cx).is_some_and(|store| {
            store.update(cx, |store, cx| {
                store.denial_blocks && store.warn_if_denied(device, cx)
            })
        })
    }

    pub fn status(&self, device: MediaDevice) -> MediaPermission {
        match device {
            MediaDevice::Microphone => self.microphone,
            MediaDevice::Camera => self.camera,
        }
    }

    pub fn is_granted(&self, device: MediaDevice) -> bool {
        self.status(device) == MediaPermission::Granted
    }

    pub fn prompt(&self) -> Option<MediaPermissionPrompt> {
        self.prompt
    }

    pub fn settings_url(&self) -> Option<&'static str> {
        self.prompt
            .and_then(|prompt| mezon_voice::media_privacy_settings_url(prompt.device()))
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let epoch = self.epoch;
        self._refresh = Some(cx.spawn(async move |this, cx| {
            let (microphone, camera) = cx
                .background_spawn(async {
                    (
                        mezon_voice::media_permission(MediaDevice::Microphone),
                        mezon_voice::media_permission(MediaDevice::Camera),
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.epoch != epoch {
                    return;
                }
                this.apply(MediaDevice::Microphone, microphone, cx);
                this.apply(MediaDevice::Camera, camera, cx);
            });
        }));
    }

    pub fn ensure(
        &mut self,
        device: MediaDevice,
        on_granted: impl FnOnce(&mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> bool {
        let status = mezon_voice::media_permission(device);
        self.ensure_with_status(device, status, on_granted, cx)
    }

    fn ensure_with_status(
        &mut self,
        device: MediaDevice,
        status: MediaPermission,
        on_granted: impl FnOnce(&mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> bool {
        self.apply_live(device, status, cx);
        let prompt = match status {
            MediaPermission::Granted => return true,
            MediaPermission::Denied if !self.denial_blocks => return true,
            MediaPermission::Undetermined => MediaPermissionPrompt::Request {
                device,
                requesting: false,
            },
            MediaPermission::Denied => MediaPermissionPrompt::Blocked(device),
        };
        if self.prompt.is_some_and(|open| {
            open.device() == device
                && std::mem::discriminant(&open) == std::mem::discriminant(&prompt)
        }) {
            return false;
        }
        self.on_granted = matches!(prompt, MediaPermissionPrompt::Request { .. })
            .then(|| Box::new(on_granted) as GrantedAction);
        self.prompt = Some(prompt);
        cx.notify();
        false
    }

    pub fn warn_if_denied(&mut self, device: MediaDevice, cx: &mut Context<Self>) -> bool {
        let status = mezon_voice::media_permission(device);
        self.warn_with_status(device, status, cx)
    }

    fn warn_with_status(
        &mut self,
        device: MediaDevice,
        status: MediaPermission,
        cx: &mut Context<Self>,
    ) -> bool {
        self.apply_live(device, status, cx);
        if status != MediaPermission::Denied {
            return false;
        }
        if self.prompt.is_none() {
            self.prompt = Some(MediaPermissionPrompt::Blocked(device));
            cx.notify();
        }
        true
    }

    pub fn request_access(&mut self, cx: &mut Context<Self>) {
        let Some(MediaPermissionPrompt::Request {
            device,
            requesting: false,
        }) = self.prompt
        else {
            return;
        };
        self.prompt = Some(MediaPermissionPrompt::Request {
            device,
            requesting: true,
        });
        cx.notify();
        mezon_voice::request_media_permission(device);
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.on_granted = None;
        if self.prompt.take().is_some() {
            cx.notify();
        }
    }

    fn apply_live(&mut self, device: MediaDevice, status: MediaPermission, cx: &mut Context<Self>) {
        self.epoch = self.epoch.wrapping_add(1);
        self.apply(device, status, cx);
    }

    fn apply(&mut self, device: MediaDevice, status: MediaPermission, cx: &mut Context<Self>) {
        let slot = match device {
            MediaDevice::Microphone => &mut self.microphone,
            MediaDevice::Camera => &mut self.camera,
        };
        let previous = std::mem::replace(slot, status);
        let prompt = self.prompt;
        match prompt.filter(|prompt| prompt.device() == device) {
            Some(MediaPermissionPrompt::Request { .. }) if status == MediaPermission::Granted => {
                self.prompt = None;
                if let Some(action) = self.on_granted.take() {
                    cx.defer(action);
                }
            }
            Some(MediaPermissionPrompt::Request { .. }) if status == MediaPermission::Denied => {
                self.on_granted = None;
                self.prompt = Some(MediaPermissionPrompt::Blocked(device));
            }
            Some(MediaPermissionPrompt::Blocked(_)) if status == MediaPermission::Granted => {
                self.prompt = None;
            }
            None if self.prompt.is_none()
                && previous == MediaPermission::Undetermined
                && status == MediaPermission::Denied =>
            {
                self.prompt = Some(MediaPermissionPrompt::Blocked(device));
            }
            _ => {}
        }
        if previous != status || prompt != self.prompt {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    fn idle_store(cx: &mut gpui::TestAppContext) -> Entity<MediaPermissionStore> {
        store_with(true, cx)
    }

    fn store_with(
        denial_blocks: bool,
        cx: &mut gpui::TestAppContext,
    ) -> Entity<MediaPermissionStore> {
        cx.update(|cx| {
            cx.new(|_| MediaPermissionStore {
                microphone: MediaPermission::Undetermined,
                camera: MediaPermission::Granted,
                prompt: None,
                on_granted: None,
                denial_blocks,
                epoch: 0,
                _changes: Task::ready(()),
                _refresh: None,
            })
        })
    }

    fn ensure(
        store: &Entity<MediaPermissionStore>,
        device: MediaDevice,
        status: MediaPermission,
        ran: &Rc<Cell<bool>>,
        cx: &mut gpui::TestAppContext,
    ) -> bool {
        let ran = ran.clone();
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.ensure_with_status(device, status, move |_| ran.set(true), cx)
            })
        })
    }

    fn apply(
        store: &Entity<MediaPermissionStore>,
        device: MediaDevice,
        status: MediaPermission,
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| store.update(cx, |store, cx| store.apply(device, status, cx)));
    }

    fn prompt(
        store: &Entity<MediaPermissionStore>,
        cx: &mut gpui::TestAppContext,
    ) -> Option<MediaPermissionPrompt> {
        cx.read(|cx| store.read(cx).prompt())
    }

    #[gpui::test]
    fn granted_device_passes_without_prompt(cx: &mut gpui::TestAppContext) {
        let store = idle_store(cx);
        let ran = Rc::new(Cell::new(false));
        assert!(ensure(
            &store,
            MediaDevice::Camera,
            MediaPermission::Granted,
            &ran,
            cx
        ));
        assert_eq!(prompt(&store, cx), None);
        assert!(!ran.get());
    }

    #[gpui::test]
    fn undetermined_device_asks_then_runs_action_once_granted(cx: &mut gpui::TestAppContext) {
        let store = idle_store(cx);
        let ran = Rc::new(Cell::new(false));
        assert!(!ensure(
            &store,
            MediaDevice::Microphone,
            MediaPermission::Undetermined,
            &ran,
            cx
        ));
        assert_eq!(
            prompt(&store, cx),
            Some(MediaPermissionPrompt::Request {
                device: MediaDevice::Microphone,
                requesting: false,
            })
        );
        cx.update(|cx| store.update(cx, |store, cx| store.request_access(cx)));
        apply(
            &store,
            MediaDevice::Microphone,
            MediaPermission::Granted,
            cx,
        );
        assert_eq!(prompt(&store, cx), None);
        assert!(ran.get());
        assert!(cx.read(|cx| store.read(cx).is_granted(MediaDevice::Microphone)));
    }

    #[gpui::test]
    fn refusing_the_system_prompt_shows_the_blocked_prompt(cx: &mut gpui::TestAppContext) {
        let store = idle_store(cx);
        let ran = Rc::new(Cell::new(false));
        ensure(
            &store,
            MediaDevice::Microphone,
            MediaPermission::Undetermined,
            &ran,
            cx,
        );
        apply(&store, MediaDevice::Microphone, MediaPermission::Denied, cx);
        assert_eq!(
            prompt(&store, cx),
            Some(MediaPermissionPrompt::Blocked(MediaDevice::Microphone))
        );
        apply(
            &store,
            MediaDevice::Microphone,
            MediaPermission::Granted,
            cx,
        );
        assert_eq!(prompt(&store, cx), None);
        assert!(!ran.get());
    }

    #[gpui::test]
    fn denied_device_shows_the_blocked_prompt(cx: &mut gpui::TestAppContext) {
        let store = idle_store(cx);
        let ran = Rc::new(Cell::new(false));
        assert!(!ensure(
            &store,
            MediaDevice::Camera,
            MediaPermission::Denied,
            &ran,
            cx
        ));
        assert_eq!(
            prompt(&store, cx),
            Some(MediaPermissionPrompt::Blocked(MediaDevice::Camera))
        );
        cx.update(|cx| store.update(cx, |store, cx| store.dismiss(cx)));
        assert_eq!(prompt(&store, cx), None);
    }

    #[gpui::test]
    fn refusal_outside_a_prompt_opens_the_blocked_prompt(cx: &mut gpui::TestAppContext) {
        let store = idle_store(cx);
        apply(&store, MediaDevice::Microphone, MediaPermission::Denied, cx);
        assert_eq!(
            prompt(&store, cx),
            Some(MediaPermissionPrompt::Blocked(MediaDevice::Microphone))
        );
    }

    #[gpui::test]
    fn revocation_outside_a_prompt_only_updates_the_status(cx: &mut gpui::TestAppContext) {
        let store = idle_store(cx);
        apply(&store, MediaDevice::Camera, MediaPermission::Denied, cx);
        assert_eq!(prompt(&store, cx), None);
        assert_eq!(
            cx.read(|cx| store.read(cx).status(MediaDevice::Camera)),
            MediaPermission::Denied
        );
    }

    #[gpui::test]
    fn another_device_change_leaves_the_open_prompt(cx: &mut gpui::TestAppContext) {
        let store = idle_store(cx);
        let ran = Rc::new(Cell::new(false));
        ensure(
            &store,
            MediaDevice::Microphone,
            MediaPermission::Undetermined,
            &ran,
            cx,
        );
        apply(&store, MediaDevice::Camera, MediaPermission::Denied, cx);
        assert_eq!(
            prompt(&store, cx).map(MediaPermissionPrompt::device),
            Some(MediaDevice::Microphone)
        );
        assert!(!ran.get());
    }

    #[gpui::test]
    fn a_second_ensure_keeps_the_pending_action(cx: &mut gpui::TestAppContext) {
        let store = idle_store(cx);
        let clicked = Rc::new(Cell::new(false));
        let held = Rc::new(Cell::new(false));
        ensure(
            &store,
            MediaDevice::Microphone,
            MediaPermission::Undetermined,
            &clicked,
            cx,
        );
        cx.update(|cx| store.update(cx, |store, cx| store.request_access(cx)));
        ensure(
            &store,
            MediaDevice::Microphone,
            MediaPermission::Undetermined,
            &held,
            cx,
        );
        assert_eq!(
            prompt(&store, cx),
            Some(MediaPermissionPrompt::Request {
                device: MediaDevice::Microphone,
                requesting: true,
            })
        );
        apply(
            &store,
            MediaDevice::Microphone,
            MediaPermission::Granted,
            cx,
        );
        assert!(clicked.get());
        assert!(!held.get());
    }

    #[gpui::test]
    fn an_advisory_denial_lets_the_device_start(cx: &mut gpui::TestAppContext) {
        let store = store_with(false, cx);
        let ran = Rc::new(Cell::new(false));
        assert!(ensure(
            &store,
            MediaDevice::Camera,
            MediaPermission::Denied,
            &ran,
            cx
        ));
        assert_eq!(prompt(&store, cx), None);
        assert!(cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.warn_with_status(MediaDevice::Camera, MediaPermission::Denied, cx)
            })
        }));
        assert_eq!(
            prompt(&store, cx),
            Some(MediaPermissionPrompt::Blocked(MediaDevice::Camera))
        );
    }
}
