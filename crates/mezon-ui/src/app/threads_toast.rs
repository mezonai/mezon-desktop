use gpui::{App, AppContext, Entity, Global, Subscription};
use mezon_store::{Settings, ThreadCreateFailReason, ThreadsEvent, ThreadsStore};

use crate::app::shell::Shell;

const THREAD_CREATE_FAILED_TOAST_KEY: &str = "thread-create-failed";
const THREAD_LEAVE_FAILED_TOAST_KEY: &str = "thread-leave-failed";

pub struct ThreadCreateToastBridge {
    _sub: Subscription,
}

struct GlobalThreadCreateToastBridge(#[allow(dead_code)] Entity<ThreadCreateToastBridge>);
impl Global for GlobalThreadCreateToastBridge {}

impl ThreadCreateToastBridge {
    pub fn init(cx: &mut App) {
        let threads = ThreadsStore::global(cx);
        let entity = cx.new(|cx| {
            let sub = cx.subscribe(&threads, |_this, _threads, event: &ThreadsEvent, cx| {
                let locale = Settings::try_global(cx)
                    .map(|settings| settings.read(cx).language.clone())
                    .unwrap_or_else(|| "en".to_string());
                match event {
                    ThreadsEvent::CreateFailed { reason } => {
                        let message = match reason {
                            ThreadCreateFailReason::ChannelLimitExceeded => {
                                mezon_i18n::t(&locale, "channel.createLimitExceeded").to_string()
                            }
                            ThreadCreateFailReason::Api(code) => {
                                mezon_i18n::api_error(&locale, *code).to_string()
                            }
                            ThreadCreateFailReason::Other => {
                                mezon_i18n::t(&locale, "common.somethingWentWrong").to_string()
                            }
                        };
                        Shell::global(cx).update(cx, |shell, cx| {
                            shell.error_once(THREAD_CREATE_FAILED_TOAST_KEY, message, cx);
                        });
                    }
                    ThreadsEvent::LeaveFailed => {
                        let message =
                            mezon_i18n::t(&locale, "common.somethingWentWrong").to_string();
                        Shell::global(cx).update(cx, |shell, cx| {
                            shell.error_once(THREAD_LEAVE_FAILED_TOAST_KEY, message, cx);
                        });
                    }
                    _ => {}
                }
            });
            Self { _sub: sub }
        });
        cx.set_global(GlobalThreadCreateToastBridge(entity));
    }
}
