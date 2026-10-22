use gpui::{App, AppContext, Context, Entity, Global, Task};

pub struct CdnAccess {
    _watch: Option<Task<()>>,
}

struct GlobalCdnAccess(Entity<CdnAccess>);
impl Global for GlobalCdnAccess {}

impl CdnAccess {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self {
            _watch: Self::spawn_watch(cx),
        });
        cx.set_global(GlobalCdnAccess(entity.clone()));
        entity
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalCdnAccess>()
            .map(|global| global.0.clone())
    }

    fn spawn_watch(cx: &mut Context<Self>) -> Option<Task<()>> {
        let mut changes = mezon_client::cdn_signature::access_changes()?;
        Some(cx.spawn(async move |this, cx| {
            while changes.changed().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        }))
    }
}

pub fn hides_media(url: &str, viewing_channel: i64) -> bool {
    let Some(source) = mezon_client::cdn_signature::channel_of_url(url) else {
        return false;
    };
    if source == viewing_channel {
        return false;
    }
    mezon_client::cdn_signature::check_access(url);
    mezon_client::cdn_signature::is_denied(url)
}

#[cfg(test)]
mod tests {
    use super::hides_media;

    #[test]
    fn only_media_from_another_channel_the_viewer_cannot_read_is_hidden() {
        crate::cdn_test_signer::install();
        let denied = "https://cdn.example/1cb164dbdac01001/2108799850765094917_secret.png";
        assert!(futures::executor::block_on(
            mezon_client::cdn_signature::access_denied(denied)
        ));
        assert!(hides_media(denied, 0x1cb1_64db_dac0_1000));
        assert!(!hides_media(denied, crate::cdn_test_signer::DENIED_CHANNEL));
        assert!(!hides_media(
            "https://cdn.example/1840673171137630208/2097582928409137152.png",
            0x1cb1_64db_dac0_1000
        ));
    }
}
