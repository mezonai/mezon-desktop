use gpui::App;
use mezon_client::transport::prioritize_avatar;

use crate::account::AccountStore;
use crate::clan::ClanList;
use crate::clan_members::{ClanMember, ClanMembersStore, User};
use crate::direct::{DirectChannel, DirectKind, DirectMessageStore};
use crate::group_members::{GroupMember, GroupMembersStore};
use crate::ids::{ChannelId, ClanId, RoleId, UserId};
use crate::presence::PresenceStore;
use crate::users_by_user::UsersByUserStore;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserProfileView {
    pub user_id: UserId,
    pub display_name: String,
    pub username: String,
    pub avatar_url: String,
    pub about_me: String,
    pub role_ids: Vec<RoleId>,
    pub create_time_seconds: u32,
    pub online: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileContext {
    Clan(ClanId),
    Direct(ChannelId),
}

impl UserProfileView {
    pub fn from_clan_member(member: &ClanMember, online: bool) -> Self {
        Self {
            user_id: member.id(),
            display_name: member.name().to_string(),
            username: member.user.username.clone(),
            avatar_url: member.avatar().to_string(),
            about_me: member.user.about_me.clone(),
            role_ids: member.role_ids.clone(),
            create_time_seconds: member.user.create_time_seconds,
            online,
        }
    }

    pub fn from_group_member(member: &GroupMember, online: bool) -> Self {
        Self {
            user_id: member.id(),
            display_name: member.name().to_string(),
            username: member.user.username.clone(),
            avatar_url: member.avatar().to_string(),
            about_me: member.user.about_me.clone(),
            role_ids: Vec::new(),
            create_time_seconds: member.user.create_time_seconds,
            online: online || member.online,
        }
    }

    pub fn from_user(user: &User, online: bool) -> Self {
        let display_name = if user.display_name.is_empty() {
            user.username.clone()
        } else {
            user.display_name.clone()
        };
        Self {
            user_id: user.id,
            display_name,
            username: user.username.clone(),
            avatar_url: user.avatar_url.clone(),
            about_me: user.about_me.clone(),
            role_ids: Vec::new(),
            create_time_seconds: user.create_time_seconds,
            online,
        }
    }

    pub fn from_direct_peer(channel: &DirectChannel, online: bool) -> Self {
        Self {
            user_id: channel.peer_user_id.unwrap_or_default(),
            display_name: channel.label.clone(),
            username: String::new(),
            avatar_url: channel.avatar.clone(),
            about_me: String::new(),
            role_ids: Vec::new(),
            create_time_seconds: 0,
            online: online || channel.online,
        }
    }
}

/// Avatar for the signed-in user in the active clan context (clan profile avatar, then account avatar).
pub fn current_user_clan_avatar(cx: &App, clan_id: Option<ClanId>) -> String {
    let store = AccountStore::global(cx).read(cx);
    let clan = store
        .clan_profile
        .as_ref()
        .filter(|profile| clan_id.is_none_or(|id| profile.clan_id == id));
    let clan_av = clan
        .and_then(|profile| profile.avatar_url.as_deref())
        .unwrap_or("");
    let user_av = store
        .account
        .as_ref()
        .and_then(|acct| acct.avatar_url.as_deref())
        .unwrap_or("");
    prioritize_avatar(clan_av, user_av)
}

/// Active clan id when the user is in a clan channel view.
pub fn active_clan_id(cx: &App) -> Option<ClanId> {
    ClanList::global(cx).read(cx).active_clan_id
}

/// Resolve a single user's profile for the popover, mirroring React's `useUserById`: a clan
/// channel reads the clan member, a DM group reads the group member, and a 1-1 DM resolves the
/// peer from the already-loaded [`DirectChannel`] (plus any cached friend record) — **no API call**.
pub fn resolve_user_profile(
    user_id: UserId,
    context: ProfileContext,
    cx: &App,
) -> Option<UserProfileView> {
    let online = PresenceStore::global(cx).read(cx).is_online(user_id);
    match context {
        ProfileContext::Clan(clan_id) => ClanMembersStore::global(cx)
            .read(cx)
            .member(clan_id, user_id)
            .map(|member| UserProfileView::from_clan_member(member, online)),
        ProfileContext::Direct(channel_id) => resolve_direct(channel_id, user_id, online, cx),
    }
}

pub fn resolve_avatar_url(user_id: UserId, context: ProfileContext, cx: &App) -> Option<String> {
    match context {
        ProfileContext::Clan(clan_id) => ClanMembersStore::global(cx)
            .read(cx)
            .member(clan_id, user_id)
            .map(|member| member.avatar().to_string()),
        ProfileContext::Direct(channel_id) => {
            let kind = DirectMessageStore::global(cx)
                .read(cx)
                .find(channel_id)
                .map(|dm| dm.kind)?;
            match kind {
                DirectKind::Group => GroupMembersStore::global(cx)
                    .read(cx)
                    .member(channel_id, user_id)
                    .map(|member| member.avatar().to_string()),
                DirectKind::Dm => {
                    if let Some(url) = UsersByUserStore::global(cx)
                        .read(cx)
                        .user(user_id)
                        .map(|user| user.avatar_url.clone())
                    {
                        return Some(url);
                    }
                    let store = DirectMessageStore::global(cx);
                    let dm = store.read(cx).find(channel_id)?;
                    (dm.peer_user_id == Some(user_id)).then(|| dm.avatar.clone())
                }
            }
        }
    }
}

fn resolve_direct(
    channel_id: ChannelId,
    user_id: UserId,
    online: bool,
    cx: &App,
) -> Option<UserProfileView> {
    let kind = DirectMessageStore::global(cx)
        .read(cx)
        .find(channel_id)
        .map(|dm| dm.kind)?;
    match kind {
        DirectKind::Group => GroupMembersStore::global(cx)
            .read(cx)
            .member(channel_id, user_id)
            .map(|member| UserProfileView::from_group_member(member, online)),
        DirectKind::Dm => {
            if let Some(view) = UsersByUserStore::global(cx)
                .read(cx)
                .user(user_id)
                .map(|user| UserProfileView::from_user(user, online))
            {
                return Some(view);
            }
            let store = DirectMessageStore::global(cx);
            let dm = store.read(cx).find(channel_id)?;
            (dm.peer_user_id == Some(user_id))
                .then(|| UserProfileView::from_direct_peer(dm, online))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UserProfileView;
    use crate::clan_members::{ClanMember, User};
    use crate::direct::{DirectChannel, DirectKind};
    use crate::group_members::GroupMember;
    use crate::ids::{ChannelId, RoleId, UserId};

    fn user(id: i64, username: &str, display: &str) -> User {
        User {
            id: UserId(id),
            username: username.into(),
            display_name: display.into(),
            avatar_url: "avatar.png".into(),
            about_me: "hi there".into(),
            create_time_seconds: 1_700_000_000,
        }
    }

    #[test]
    fn clan_member_prefers_nick_and_keeps_roles() {
        let member = ClanMember {
            user: user(7, "bob", "Bobby"),
            clan_nick: "Boss".into(),
            clan_avatar: "clan_avatar.png".into(),
            role_ids: vec![RoleId(3), RoleId(4)],
            online: false,
        };
        let view = UserProfileView::from_clan_member(&member, true);
        assert_eq!(view.user_id, UserId(7));
        assert_eq!(view.display_name, "Boss");
        assert_eq!(view.username, "bob");
        assert_eq!(view.avatar_url, "clan_avatar.png");
        assert_eq!(view.about_me, "hi there");
        assert_eq!(view.role_ids, vec![RoleId(3), RoleId(4)]);
        assert_eq!(view.create_time_seconds, 1_700_000_000);
        assert!(view.online);
    }

    #[test]
    fn group_member_has_no_roles_and_merges_online() {
        let member = GroupMember {
            user: user(9, "kay", ""),
            online: true,
        };
        let view = UserProfileView::from_group_member(&member, false);
        assert_eq!(view.display_name, "kay");
        assert!(view.role_ids.is_empty());
        assert!(view.online);
    }

    #[test]
    fn direct_peer_resolves_from_channel_without_api() {
        let channel = DirectChannel {
            id: ChannelId(100),
            label: "Alice".into(),
            kind: DirectKind::Dm,
            avatar: "alice.png".into(),
            peer_user_id: Some(UserId(5)),
            peer_username: "alice".into(),
            creator_id: None,
            online: true,
            member_count: 2,
            unread_count: 0,
            last_sent_timestamp: 0,
            last_seen_timestamp: 0,
        };
        let view = UserProfileView::from_direct_peer(&channel, false);
        assert_eq!(view.user_id, UserId(5));
        assert_eq!(view.display_name, "Alice");
        assert!(view.username.is_empty());
        assert_eq!(view.avatar_url, "alice.png");
        assert!(view.online);
    }
}
