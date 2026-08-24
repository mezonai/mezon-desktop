use gpui::{App, AppContext, AsyncApp, Context, Entity, Global, Subscription, Task, WeakEntity};

use crate::channel::ChannelList;
use crate::clan::{ClanEvent, ClanList};
use crate::clan_members::ClanMembersStore;
use crate::emoji::EmojiStore;
use crate::ids::ClanId;
use crate::notification_setting::NotificationSettingStore;
use crate::permissions::PermissionStore;
use crate::roles::RolesStore;
use crate::sticker::StickerStore;

pub struct ClanLoadScheduler {
    generation: u64,
    loading_clan: Option<ClanId>,
    load_task: Option<Task<()>>,
    _clan_sub: Subscription,
}

struct GlobalClanLoadScheduler(Entity<ClanLoadScheduler>);
impl Global for GlobalClanLoadScheduler {}

impl ClanLoadScheduler {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(Self::new);
        cx.set_global(GlobalClanLoadScheduler(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalClanLoadScheduler>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalClanLoadScheduler>()
            .map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        self.cancel_in_flight(cx);
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let clan_sub = cx.subscribe(
            &ClanList::global(cx),
            |this, _clan, event, cx| match event {
                ClanEvent::ActiveClanChanged(active) => match active {
                    Some(clan_id) => this.start(*clan_id, cx),
                    None => this.reset(cx),
                },
                ClanEvent::Joined(clan_id) => {
                    if ClanList::global(cx).read(cx).active_clan_id == Some(*clan_id) {
                        this.start(*clan_id, cx);
                    }
                }
                ClanEvent::Deleted(_) | ClanEvent::OwnerChanged { .. } => {}
            },
        );

        Self {
            generation: 0,
            loading_clan: None,
            load_task: None,
            _clan_sub: clan_sub,
        }
    }

    fn cancel_in_flight(&mut self, cx: &mut Context<Self>) {
        self.load_task = None;
        if let Some(previous) = self.loading_clan.take() {
            ChannelList::global(cx).update(cx, |channels, _| channels.cancel_badge_seed(previous));
        }
    }

    fn start(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.cancel_in_flight(cx);
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.loading_clan = Some(clan_id);

        self.load_task = Some(cx.spawn(async move |this, cx| {
            cx.update(|cx| {
                ChannelList::global(cx)
                    .update(cx, |channels, cx| channels.load_for_clan(clan_id, cx));
            });

            cx.update(|cx| {
                ChannelList::global(cx).update(cx, |channels, cx| channels.seed_badges(clan_id, cx))
            })
            .await;
            if !is_current(&this, cx, generation) {
                return;
            }

            cx.update(|cx| {
                ClanMembersStore::global(cx)
                    .update(cx, |store, cx| store.ensure_loaded_task(clan_id, cx))
            })
            .await;
            if !is_current(&this, cx, generation) {
                return;
            }

            let deferred = cx.update(|cx| {
                [
                    RolesStore::global(cx)
                        .update(cx, |store, cx| store.ensure_loaded_task(clan_id, cx)),
                    PermissionStore::global(cx).update(cx, |store, cx| {
                        store.load_clan_permissions_task(clan_id, cx)
                    }),
                    EmojiStore::global(cx).update(cx, |store, cx| store.ensure_loaded_task(cx)),
                    StickerStore::global(cx).update(cx, |store, cx| store.ensure_loaded_task(cx)),
                ]
            });
            for task in deferred {
                task.await;
            }
            if !is_current(&this, cx, generation) {
                return;
            }

            cx.update(|cx| {
                NotificationSettingStore::global(cx)
                    .update(cx, |store, cx| store.prefetch_clan(clan_id, cx));
            });

            this.update(cx, |this, _| {
                if this.generation == generation {
                    this.loading_clan = None;
                }
            })
            .ok();
        }));
    }
}

fn is_current(this: &WeakEntity<ClanLoadScheduler>, cx: &mut AsyncApp, generation: u64) -> bool {
    this.update(cx, |this, _| this.generation == generation)
        .unwrap_or(false)
}
