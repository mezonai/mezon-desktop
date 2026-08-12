use gpui::{App, AppContext, Entity, Global, Subscription};
use mezon_store::{WalletEvent, WalletStore};

use crate::app::shell::Shell;

pub struct WalletToastBridge {
    _sub: Subscription,
}

struct GlobalWalletToastBridge(#[allow(dead_code)] Entity<WalletToastBridge>);
impl Global for GlobalWalletToastBridge {}

impl WalletToastBridge {
    pub fn init(cx: &mut App) {
        let wallet = WalletStore::global(cx);
        let entity = cx.new(|cx| {
            let sub = cx.subscribe(
                &wallet,
                |_this, _wallet, event: &WalletEvent, cx| match event {
                    WalletEvent::CoffeeSent => {
                        Shell::global(cx).update(cx, |shell, cx| shell.success("Coffee sent", cx));
                    }
                    WalletEvent::SendFailed { message } | WalletEvent::EnableFailed { message } => {
                        let message = message.clone();
                        Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                    }
                    _ => {}
                },
            );
            Self { _sub: sub }
        });
        cx.set_global(GlobalWalletToastBridge(entity));
    }
}
