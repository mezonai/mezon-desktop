use gpui::App;
use mezon_store::{
    AccountStore, BadgeService, ClanId, ClanMembersStore, StreamMember, UserId, UsersByUserStore,
    VoiceMember, user_profile::ProfileContext,
};

pub(crate) struct ResolvedMemberDisplay {
    pub name: String,
    pub avatar_src: String,
    pub avatar_raw: String,
}

pub(crate) fn resolve_display(
    cx: &App,
    clan_id: Option<ClanId>,
    m: &VoiceMember,
) -> ResolvedMemberDisplay {
    resolve_user_display(cx, clan_id, m.user_id, &m.display_name, &m.avatar_url)
}

pub(crate) fn resolve_stream_display(
    cx: &App,
    clan_id: Option<ClanId>,
    m: &StreamMember,
) -> ResolvedMemberDisplay {
    resolve_user_display(cx, clan_id, m.user_id, &m.display_name, "")
}

fn resolve_user_display(
    cx: &App,
    clan_id: Option<ClanId>,
    user_id: UserId,
    fallback_name: &str,
    fallback_avatar: &str,
) -> ResolvedMemberDisplay {
    let mut name = fallback_name.to_string();
    let mut avatar_raw = fallback_avatar.to_string();

    if let Some(clan_id) = clan_id
        && let Some(store) = ClanMembersStore::try_global(cx)
        && let Some(member) = store.read(cx).member(clan_id, user_id)
    {
        if !member.name().is_empty() {
            name = member.name().to_string();
        }
        if !member.avatar().is_empty() {
            avatar_raw = member.avatar().to_string();
        }
    }

    if (name.is_empty() || avatar_raw.is_empty())
        && let Some(store) = UsersByUserStore::try_global(cx)
        && let Some(user) = store.read(cx).user(user_id)
    {
        if name.is_empty() {
            name = if !user.display_name.is_empty() {
                user.display_name.clone()
            } else {
                user.username.clone()
            };
        }
        if avatar_raw.is_empty() && !user.avatar_url.is_empty() {
            avatar_raw = user.avatar_url.clone();
        }
    }

    if (name.is_empty() || avatar_raw.is_empty())
        && BadgeService::global(cx).read(cx).current_user_id(cx) == Some(user_id)
        && let Some(account) = AccountStore::try_global(cx)
        && let Some(me) = account.read(cx).account.as_ref()
    {
        if name.is_empty() {
            name = if me.display_name.is_empty() {
                me.username.clone()
            } else {
                me.display_name.clone()
            };
        }
        if avatar_raw.is_empty()
            && let Some(url) = me.avatar_url.as_ref().filter(|u| !u.is_empty())
        {
            avatar_raw = url.clone();
        }
    }

    if avatar_raw.is_empty()
        && let Some(clan_id) = clan_id
        && let Some(url) =
            mezon_store::resolve_avatar_url(user_id, ProfileContext::Clan(clan_id), cx)
    {
        avatar_raw = url;
    }

    let avatar_src = if avatar_raw.is_empty() {
        String::new()
    } else {
        crate::util::imgproxy::avatar_url(cx, &avatar_raw)
    };

    ResolvedMemberDisplay {
        name,
        avatar_src,
        avatar_raw,
    }
}
