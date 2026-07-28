use gpui::App;
use mezon_store::{AccountStore, BadgeService, ClanId, ClanMembersStore, VoiceMember};

pub(crate) fn resolve_display(
    cx: &App,
    clan_id: Option<ClanId>,
    m: &VoiceMember,
) -> (String, String) {
    let mut name = m.display_name.clone();
    let mut avatar_url = m.avatar_url.clone();
    if let Some(clan_id) = clan_id
        && let Some(store) = ClanMembersStore::try_global(cx)
        && let Some(member) = store.read(cx).member(clan_id, m.user_id)
    {
        if !member.name().is_empty() {
            name = member.name().to_string();
        }
        if !member.avatar().is_empty() {
            avatar_url = member.avatar().to_string();
        }
    }
    if name.is_empty()
        && BadgeService::global(cx).read(cx).current_user_id(cx) == Some(m.user_id)
        && let Some(account) = AccountStore::try_global(cx)
        && let Some(me) = account.read(cx).account.as_ref()
    {
        name = if me.display_name.is_empty() {
            me.username.clone()
        } else {
            me.display_name.clone()
        };
        if avatar_url.is_empty()
            && let Some(url) = me.avatar_url.as_ref().filter(|u| !u.is_empty())
        {
            avatar_url = url.clone();
        }
    }
    if !avatar_url.is_empty() {
        avatar_url = crate::util::imgproxy::avatar_url(cx, &avatar_url);
    }
    (name, avatar_url)
}
