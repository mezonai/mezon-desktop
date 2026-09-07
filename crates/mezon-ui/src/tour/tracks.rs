use super::anchor::TourAnchor;
use crate::clan::settings::ClanSettingsPage;
use crate::router::Route;

pub const TOUR_VERSION: u32 = 1;
pub const CLAN_TRACK_ID: &str = "start";
pub const DM_TRACK_ID: &str = "dmstart";
pub const CLAN_SETTINGS_TRACK_ID: &str = "clansettings";

pub struct TourStep {
    pub anchor: Option<TourAnchor>,
    pub title_key: &'static str,
    pub body_key: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackPrecondition {
    Conversation,
    OpenConversation,
    ClanChannel,
    DirectSpace,
    ClanSettings,
}

impl TrackPrecondition {
    pub fn is_met(self, route: &Route) -> bool {
        match self {
            Self::Conversation => matches!(
                route,
                Route::Channel { .. }
                    | Route::Thread { .. }
                    | Route::DirectMessage { .. }
                    | Route::Chat
                    | Route::Direct
            ),
            Self::ClanChannel => {
                matches!(route, Route::Channel { .. } | Route::Thread { .. })
            }
            Self::OpenConversation => matches!(
                route,
                Route::Channel { .. } | Route::Thread { .. } | Route::DirectMessage { .. }
            ),
            Self::ClanSettings => matches!(route, Route::ClanSettings { .. }),
            Self::DirectSpace => matches!(
                route,
                Route::Direct | Route::DirectMessage { .. } | Route::Friends | Route::Chat
            ),
        }
    }
}

pub struct TourTrack {
    pub id: &'static str,
    pub name_key: &'static str,
    pub summary_key: &'static str,
    pub precondition: TrackPrecondition,
    pub steps: &'static [TourStep],
}

pub fn track(id: &str) -> Option<&'static TourTrack> {
    TRACKS.iter().find(|track| track.id == id)
}

pub fn core_track_for(route: &Route) -> Option<&'static TourTrack> {
    let id = match route {
        Route::Channel { .. }
        | Route::Thread { .. }
        | Route::Canvas { .. }
        | Route::ClanChannels { .. }
        | Route::ClanMembers { .. }
        | Route::ClanGuide { .. } => CLAN_TRACK_ID,
        Route::ClanSettings { .. } => CLAN_SETTINGS_TRACK_ID,
        Route::Chat
        | Route::Direct
        | Route::DirectMessage { .. }
        | Route::Friends
        | Route::AddFriend { .. } => DM_TRACK_ID,
        _ => return None,
    };
    track(id)
}

pub static TRACKS: &[TourTrack] = &[
    TourTrack {
        id: CLAN_TRACK_ID,
        name_key: "tour.start.name",
        summary_key: "tour.start.summary",
        precondition: TrackPrecondition::ClanChannel,
        steps: &[
            TourStep {
                anchor: Some(TourAnchor::ClanRail),
                title_key: "tour.start.s1.title",
                body_key: "tour.start.s1.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ChannelList),
                title_key: "tour.start.s2.title",
                body_key: "tour.start.s2.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ClanHeader),
                title_key: "tour.start.clanmenu.title",
                body_key: "tour.start.clanmenu.body",
            },
            TourStep {
                anchor: Some(TourAnchor::MessageTimeline),
                title_key: "tour.start.s3.title",
                body_key: "tour.start.s3.body",
            },
            TourStep {
                anchor: Some(TourAnchor::Composer),
                title_key: "tour.start.s4.title",
                body_key: "tour.start.s4.body",
            },
            TourStep {
                anchor: Some(TourAnchor::MemberList),
                title_key: "tour.start.s5.title",
                body_key: "tour.start.s5.body",
            },
            TourStep {
                anchor: Some(TourAnchor::UserInfoBar),
                title_key: "tour.start.s6.title",
                body_key: "tour.start.s6.body",
            },
            TourStep {
                anchor: None,
                title_key: "tour.start.s7.title",
                body_key: "tour.start.s7.body",
            },
        ],
    },
    TourTrack {
        id: DM_TRACK_ID,
        name_key: "tour.dm.name",
        summary_key: "tour.dm.summary",
        precondition: TrackPrecondition::DirectSpace,
        steps: &[
            TourStep {
                anchor: Some(TourAnchor::ClanRail),
                title_key: "tour.start.s1.title",
                body_key: "tour.start.s1.body",
            },
            TourStep {
                anchor: Some(TourAnchor::DirectList),
                title_key: "tour.dm.s2.title",
                body_key: "tour.dm.s2.body",
            },
            TourStep {
                anchor: Some(TourAnchor::FriendsButton),
                title_key: "tour.dm.s3.title",
                body_key: "tour.dm.s3.body",
            },
            TourStep {
                anchor: Some(TourAnchor::AddFriendButton),
                title_key: "tour.dm.addfriend.title",
                body_key: "tour.dm.addfriend.body",
            },
            TourStep {
                anchor: Some(TourAnchor::MessageTimeline),
                title_key: "tour.dm.s4.title",
                body_key: "tour.dm.s4.body",
            },
            TourStep {
                anchor: Some(TourAnchor::Composer),
                title_key: "tour.start.s4.title",
                body_key: "tour.start.s4.body",
            },
            TourStep {
                anchor: Some(TourAnchor::UserInfoBar),
                title_key: "tour.start.s6.title",
                body_key: "tour.start.s6.body",
            },
            TourStep {
                anchor: None,
                title_key: "tour.start.s7.title",
                body_key: "tour.start.s7.body",
            },
        ],
    },
    TourTrack {
        id: "messaging",
        name_key: "tour.messaging.name",
        summary_key: "tour.messaging.summary",
        precondition: TrackPrecondition::OpenConversation,
        steps: &[
            TourStep {
                anchor: Some(TourAnchor::Composer),
                title_key: "tour.messaging.s1.title",
                body_key: "tour.messaging.s1.body",
            },
            TourStep {
                anchor: Some(TourAnchor::Composer),
                title_key: "tour.messaging.s2.title",
                body_key: "tour.messaging.s2.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ComposerTools),
                title_key: "tour.messaging.s3.title",
                body_key: "tour.messaging.s3.body",
            },
            TourStep {
                anchor: None,
                title_key: "tour.messaging.s4.title",
                body_key: "tour.messaging.s4.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ChannelHeaderTools),
                title_key: "tour.messaging.s5.title",
                body_key: "tour.messaging.s5.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ChannelHeaderTools),
                title_key: "tour.messaging.s6.title",
                body_key: "tour.messaging.s6.body",
            },
        ],
    },
    TourTrack {
        id: "voice",
        name_key: "tour.voice.name",
        summary_key: "tour.voice.summary",
        precondition: TrackPrecondition::ClanChannel,
        steps: &[
            TourStep {
                anchor: Some(TourAnchor::ChannelList),
                title_key: "tour.voice.s1.title",
                body_key: "tour.voice.s1.body",
            },
            TourStep {
                anchor: Some(TourAnchor::VoiceControls),
                title_key: "tour.voice.s2.title",
                body_key: "tour.voice.s2.body",
            },
            TourStep {
                anchor: Some(TourAnchor::VoiceControls),
                title_key: "tour.voice.s3.title",
                body_key: "tour.voice.s3.body",
            },
            TourStep {
                anchor: None,
                title_key: "tour.voice.s4.title",
                body_key: "tour.voice.s4.body",
            },
            TourStep {
                anchor: Some(TourAnchor::UserInfoBar),
                title_key: "tour.voice.s5.title",
                body_key: "tour.voice.s5.body",
            },
        ],
    },
    TourTrack {
        id: "wallet",
        name_key: "tour.wallet.name",
        summary_key: "tour.wallet.summary",
        precondition: TrackPrecondition::Conversation,
        steps: &[
            TourStep {
                anchor: Some(TourAnchor::UserInfoBar),
                title_key: "tour.wallet.s1.title",
                body_key: "tour.wallet.s1.body",
            },
            TourStep {
                anchor: None,
                title_key: "tour.wallet.s2.title",
                body_key: "tour.wallet.s2.body",
            },
            TourStep {
                anchor: None,
                title_key: "tour.wallet.s3.title",
                body_key: "tour.wallet.s3.body",
            },
            TourStep {
                anchor: None,
                title_key: "tour.wallet.s4.title",
                body_key: "tour.wallet.s4.body",
            },
        ],
    },
    TourTrack {
        id: "toolbar",
        name_key: "tour.toolbar.name",
        summary_key: "tour.toolbar.summary",
        precondition: TrackPrecondition::OpenConversation,
        steps: &[
            TourStep {
                anchor: Some(TourAnchor::ChannelHeaderTools),
                title_key: "tour.toolbar.s1.title",
                body_key: "tour.toolbar.s1.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ChannelHeaderTools),
                title_key: "tour.toolbar.s2.title",
                body_key: "tour.toolbar.s2.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ChannelHeaderTools),
                title_key: "tour.toolbar.s3.title",
                body_key: "tour.toolbar.s3.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ChannelHeaderTools),
                title_key: "tour.toolbar.s4.title",
                body_key: "tour.toolbar.s4.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ChannelHeaderSearch),
                title_key: "tour.toolbar.s5.title",
                body_key: "tour.toolbar.s5.body",
            },
        ],
    },
    TourTrack {
        id: "friends",
        name_key: "tour.friends.name",
        summary_key: "tour.friends.summary",
        precondition: TrackPrecondition::DirectSpace,
        steps: &[
            TourStep {
                anchor: Some(TourAnchor::FriendsButton),
                title_key: "tour.friends.s1.title",
                body_key: "tour.friends.s1.body",
            },
            TourStep {
                anchor: Some(TourAnchor::FriendsPage),
                title_key: "tour.friends.s2.title",
                body_key: "tour.friends.s2.body",
            },
            TourStep {
                anchor: Some(TourAnchor::FriendsPage),
                title_key: "tour.friends.s3.title",
                body_key: "tour.friends.s3.body",
            },
            TourStep {
                anchor: Some(TourAnchor::DirectList),
                title_key: "tour.friends.s4.title",
                body_key: "tour.friends.s4.body",
            },
        ],
    },
    TourTrack {
        id: CLAN_SETTINGS_TRACK_ID,
        name_key: "tour.clansettings.name",
        summary_key: "tour.clansettings.summary",
        precondition: TrackPrecondition::ClanSettings,
        steps: &[
            TourStep {
                anchor: Some(TourAnchor::ClanSettingsNav),
                title_key: "tour.clansettings.s1.title",
                body_key: "tour.clansettings.s1.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ClanSettingsRow(ClanSettingsPage::Overview)),
                title_key: "tour.clansettings.overview.title",
                body_key: "tour.clansettings.overview.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ClanSettingsRow(ClanSettingsPage::Roles)),
                title_key: "tour.clansettings.roles.title",
                body_key: "tour.clansettings.roles.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ClanSettingsRow(ClanSettingsPage::CategoryOrder)),
                title_key: "tour.clansettings.categoryorder.title",
                body_key: "tour.clansettings.categoryorder.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ClanSettingsRow(
                    ClanSettingsPage::ArchivedChannels,
                )),
                title_key: "tour.clansettings.archived.title",
                body_key: "tour.clansettings.archived.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ClanSettingsRow(ClanSettingsPage::Emoji)),
                title_key: "tour.clansettings.emoji.title",
                body_key: "tour.clansettings.emoji.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ClanSettingsRow(ClanSettingsPage::ImageStickers)),
                title_key: "tour.clansettings.stickers.title",
                body_key: "tour.clansettings.stickers.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ClanSettingsRow(ClanSettingsPage::VoiceStickers)),
                title_key: "tour.clansettings.sounds.title",
                body_key: "tour.clansettings.sounds.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ClanSettingsRow(ClanSettingsPage::Integrations)),
                title_key: "tour.clansettings.integrations.title",
                body_key: "tour.clansettings.integrations.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ClanSettingsRow(ClanSettingsPage::AuditLog)),
                title_key: "tour.clansettings.auditlog.title",
                body_key: "tour.clansettings.auditlog.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ClanSettingsRow(ClanSettingsPage::Onboarding)),
                title_key: "tour.clansettings.onboarding.title",
                body_key: "tour.clansettings.onboarding.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ClanSettingsRow(ClanSettingsPage::ClanCommunity)),
                title_key: "tour.clansettings.community.title",
                body_key: "tour.clansettings.community.body",
            },
        ],
    },
    TourTrack {
        id: "clanadmin",
        name_key: "tour.clanadmin.name",
        summary_key: "tour.clanadmin.summary",
        precondition: TrackPrecondition::ClanChannel,
        steps: &[
            TourStep {
                anchor: Some(TourAnchor::ClanHeader),
                title_key: "tour.clanadmin.s1.title",
                body_key: "tour.clanadmin.s1.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ChannelList),
                title_key: "tour.clanadmin.s2.title",
                body_key: "tour.clanadmin.s2.body",
            },
            TourStep {
                anchor: Some(TourAnchor::CreateChannel),
                title_key: "tour.clanadmin.s2a.title",
                body_key: "tour.clanadmin.s2a.body",
            },
            TourStep {
                anchor: Some(TourAnchor::CreateChannel),
                title_key: "tour.clanadmin.s2b.title",
                body_key: "tour.clanadmin.s2b.body",
            },
            TourStep {
                anchor: Some(TourAnchor::CreateChannel),
                title_key: "tour.clanadmin.s2c.title",
                body_key: "tour.clanadmin.s2c.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ClanHeader),
                title_key: "tour.clanadmin.s3.title",
                body_key: "tour.clanadmin.s3.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ClanMembersRow),
                title_key: "tour.clanadmin.s5.title",
                body_key: "tour.clanadmin.s5.body",
            },
            TourStep {
                anchor: Some(TourAnchor::ClanHeader),
                title_key: "tour.clanadmin.s4.title",
                body_key: "tour.clanadmin.s4.body",
            },
            TourStep {
                anchor: None,
                title_key: "tour.clanadmin.s6.title",
                body_key: "tour.clanadmin.s6.body",
            },
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use mezon_store::{ChannelId, ClanId};

    #[test]
    fn every_track_has_a_step() {
        assert!(!TRACKS.is_empty());
        for track in TRACKS {
            assert!(!track.steps.is_empty(), "track {} is empty", track.id);
        }
    }

    #[test]
    fn track_ids_are_unique() {
        let mut ids: Vec<_> = TRACKS.iter().map(|track| track.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len());
    }

    #[test]
    fn core_track_is_reachable_and_fully_anchored() {
        let core = track(CLAN_TRACK_ID).expect("clan core track");
        assert_eq!(core.precondition, TrackPrecondition::ClanChannel);
        assert!(
            core.steps
                .iter()
                .filter(|step| step.anchor.is_some())
                .count()
                >= 6
        );
    }

    fn guaranteed_on_route(precondition: TrackPrecondition) -> &'static [TourAnchor] {
        match precondition {
            TrackPrecondition::Conversation => &[TourAnchor::ClanRail, TourAnchor::UserInfoBar],
            TrackPrecondition::DirectSpace => &[
                TourAnchor::ClanRail,
                TourAnchor::DirectList,
                TourAnchor::UserInfoBar,
            ],
            TrackPrecondition::OpenConversation => &[
                TourAnchor::ClanRail,
                TourAnchor::UserInfoBar,
                TourAnchor::ChannelHeaderTools,
                TourAnchor::Composer,
            ],
            TrackPrecondition::ClanChannel => &[
                TourAnchor::ClanRail,
                TourAnchor::ClanHeader,
                TourAnchor::ChannelList,
                TourAnchor::UserInfoBar,
            ],
            TrackPrecondition::ClanSettings => &[TourAnchor::ClanSettingsNav],
        }
    }

    #[test]
    fn every_anchor_a_track_uses_is_probed_somewhere_in_the_ui() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sources = String::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read src") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && !path.ends_with("tour/tracks.rs")
                    && !path.ends_with("tour/anchor.rs")
                {
                    sources.push_str(&std::fs::read_to_string(&path).expect("read source"));
                }
            }
        }
        for track in TRACKS {
            for step in track.steps {
                let Some(anchor) = step.anchor else { continue };
                let rendered = format!("{anchor:?}");
                let variant = rendered.split('(').next().expect("variant name");
                let needle = format!("TourAnchor::{variant}");
                assert!(
                    sources.contains(&needle),
                    "{} step {} points at {anchor:?} but no probe() call site mentions it; \
                     deleting a probe must fail this test, not pass silently",
                    track.id,
                    step.title_key
                );
            }
        }
    }

    #[test]
    fn every_track_keeps_a_step_its_route_guarantees() {
        for track in TRACKS {
            let guaranteed = guaranteed_on_route(track.precondition);
            let survives = track.steps.iter().any(|step| {
                step.anchor
                    .is_some_and(|anchor| guaranteed.contains(&anchor))
            });
            assert!(
                survives,
                "track {} keeps no anchored step its own route guarantees, so it can \
                 degrade to nothing but centered cards",
                track.id
            );
        }
    }

    #[test]
    fn a_core_track_exists_for_both_contexts() {
        let channel = Route::Channel {
            clan_id: ClanId(1),
            channel_id: ChannelId(2),
        };
        let dm = Route::DirectMessage {
            direct_id: ChannelId(3),
            message_type: String::new(),
        };
        assert_eq!(core_track_for(&channel).map(|t| t.id), Some(CLAN_TRACK_ID));
        assert_eq!(core_track_for(&dm).map(|t| t.id), Some(DM_TRACK_ID));
        assert_eq!(
            core_track_for(&Route::Friends).map(|t| t.id),
            Some(DM_TRACK_ID)
        );
        assert!(core_track_for(&Route::SettingsAccount).is_none());
    }

    #[test]
    fn every_core_step_is_anchored_except_the_closing_card() {
        for id in [CLAN_TRACK_ID, DM_TRACK_ID] {
            let core = track(id).expect("core track");
            let unanchored = core.steps.iter().filter(|s| s.anchor.is_none()).count();
            assert_eq!(unanchored, 1, "{id} should only end with a centered card");
        }
    }

    #[test]
    fn preconditions_gate_on_route() {
        let channel = Route::Channel {
            clan_id: ClanId(1),
            channel_id: ChannelId(2),
        };
        let dm = Route::DirectMessage {
            direct_id: ChannelId(3),
            message_type: String::new(),
        };
        let friends = Route::Friends;

        assert!(TrackPrecondition::Conversation.is_met(&dm));
        assert!(!TrackPrecondition::Conversation.is_met(&friends));
        assert!(TrackPrecondition::ClanChannel.is_met(&channel));
        assert!(!TrackPrecondition::ClanChannel.is_met(&dm));
        assert!(TrackPrecondition::DirectSpace.is_met(&dm));
        assert!(TrackPrecondition::DirectSpace.is_met(&friends));
        assert!(!TrackPrecondition::DirectSpace.is_met(&channel));
    }
}
