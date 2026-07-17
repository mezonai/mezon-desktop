use std::time::Duration;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, SharedString, Task};
use serde::Deserialize;

use crate::config::AppConfig;

const GIF_LIMIT: u32 = 30;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone)]
pub struct GifCategory {
    pub name: SharedString,
    pub searchterm: SharedString,
    pub image: SharedString,
}

#[derive(Debug, Clone)]
pub struct Gif {
    pub url: SharedString,
    pub preview_url: SharedString,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub enum GifEvent {
    Changed,
}

#[derive(Deserialize)]
struct KlipyCategoriesResponse {
    #[serde(default)]
    data: KlipyCategoriesData,
}

#[derive(Deserialize, Default)]
struct KlipyCategoriesData {
    #[serde(default)]
    categories: Vec<KlipyCategory>,
}

#[derive(Deserialize)]
struct KlipyCategory {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    preview_url: Option<String>,
}

#[derive(Deserialize)]
struct KlipyGifsResponse {
    #[serde(default)]
    data: KlipyGifsData,
}

#[derive(Deserialize, Default)]
struct KlipyGifsData {
    #[serde(default)]
    data: Vec<KlipyGif>,
}

#[derive(Deserialize)]
struct KlipyGif {
    #[serde(default)]
    file: KlipyFile,
}

#[derive(Deserialize, Default)]
struct KlipyFile {
    xs: Option<KlipyVariant>,
    sm: Option<KlipyVariant>,
    md: Option<KlipyVariant>,
    hd: Option<KlipyVariant>,
}

#[derive(Deserialize)]
struct KlipyVariant {
    gif: Option<KlipyMedia>,
}

#[derive(Deserialize)]
struct KlipyMedia {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
}

impl KlipyMedia {
    fn url(&self) -> &str {
        self.url.as_deref().unwrap_or_default()
    }
}

impl KlipyFile {
    fn gif_media(variant: Option<&KlipyVariant>) -> Option<&KlipyMedia> {
        variant
            .and_then(|v| v.gif.as_ref())
            .filter(|media| !media.url().is_empty())
    }

    fn shareable(&self) -> Option<&KlipyMedia> {
        Self::gif_media(self.xs.as_ref())
            .or_else(|| Self::gif_media(self.sm.as_ref()))
            .or_else(|| Self::gif_media(self.md.as_ref()))
            .or_else(|| Self::gif_media(self.hd.as_ref()))
    }
}

struct KlipyConfig {
    key: String,
    base_url: String,
}

impl KlipyConfig {
    fn from_app(cx: &App) -> Self {
        let config = AppConfig::global(cx);
        Self {
            key: config.klipy_key.clone(),
            base_url: config.klipy_base_url.clone(),
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!(
            "{}/{}/gifs/{path}",
            self.base_url.trim_end_matches('/'),
            self.key
        )
    }

    fn categories_url(&self) -> String {
        self.endpoint("categories")
    }

    fn featured_url(&self) -> String {
        format!(
            "{}?page=1&per_page={GIF_LIMIT}&format_filter=gif",
            self.endpoint("trending")
        )
    }

    fn search_url(&self, query: &str) -> String {
        format!(
            "{}?page=1&per_page={GIF_LIMIT}&q={}&format_filter=gif",
            self.endpoint("search"),
            encode_query(query)
        )
    }
}

pub struct GifStore {
    categories: Vec<GifCategory>,
    featured: Vec<Gif>,
    search_results: Vec<Gif>,
    loaded: bool,
    loading: bool,
    featured_loading: bool,
    searching: bool,
    search_generation: u64,
    config: KlipyConfig,
    _base_task: Option<Task<()>>,
    _featured_task: Option<Task<()>>,
    _search_task: Option<Task<()>>,
}

struct GlobalGifStore(Entity<GifStore>);
impl Global for GlobalGifStore {}

impl EventEmitter<GifEvent> for GifStore {}

impl GifStore {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(Self::new);
        cx.set_global(GlobalGifStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalGifStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalGifStore>().map(|g| g.0.clone())
    }

    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            categories: Vec::new(),
            featured: Vec::new(),
            search_results: Vec::new(),
            loaded: false,
            loading: false,
            featured_loading: false,
            searching: false,
            search_generation: 0,
            config: KlipyConfig::from_app(cx),
            _base_task: None,
            _featured_task: None,
            _search_task: None,
        }
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.loaded && !self.loading {
            self.fetch_base(cx);
        }
    }

    pub fn ensure_featured(&mut self, cx: &mut Context<Self>) {
        if !self.featured.is_empty() || self.loading || self.featured_loading {
            return;
        }
        if self.config.key.is_empty() {
            tracing::error!(
                "klipy api key is empty: set NX_CHAT_APP_API_KLIPY_KEY at build time (.env or CI)"
            );
            return;
        }
        self.featured_loading = true;
        cx.notify();
        let featured_url = self.config.featured_url();
        self._featured_task = Some(cx.spawn(async move |this, cx| {
            let featured = fetch_gifs(&featured_url).await;
            let _ = this.update(cx, |this, cx| {
                this.featured_loading = false;
                if let Some(featured) = featured {
                    this.featured = featured;
                }
                cx.emit(GifEvent::Changed);
                cx.notify();
            });
        }));
    }

    pub fn is_featured_loading(&self) -> bool {
        self.loading || self.featured_loading
    }

    fn fetch_base(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        if self.config.key.is_empty() {
            tracing::error!(
                "klipy api key is empty: set NX_CHAT_APP_API_KLIPY_KEY at build time (.env or CI)"
            );
            return;
        }
        self.loading = true;
        let categories_url = self.config.categories_url();
        let featured_url = self.config.featured_url();
        self._base_task = Some(cx.spawn(async move |this, cx| {
            let categories = fetch_categories(&categories_url).await;
            let featured = fetch_gifs(&featured_url).await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                if let Some(categories) = categories {
                    this.categories = categories;
                }
                if let Some(featured) = featured {
                    this.featured = featured;
                }
                this.loaded = !this.categories.is_empty() || !this.featured.is_empty();
                cx.emit(GifEvent::Changed);
                cx.notify();
            });
        }));
    }

    pub fn search(&mut self, query: String, cx: &mut Context<Self>) {
        self.start_search(query, SEARCH_DEBOUNCE, cx);
    }

    pub fn search_now(&mut self, query: String, cx: &mut Context<Self>) {
        self.start_search(query, Duration::ZERO, cx);
    }

    fn start_search(&mut self, query: String, debounce: Duration, cx: &mut Context<Self>) {
        let trimmed = query.trim().to_string();
        self.search_generation = self.search_generation.wrapping_add(1);
        if trimmed.is_empty() {
            self.searching = false;
            if !self.search_results.is_empty() {
                self.search_results.clear();
                cx.emit(GifEvent::Changed);
            }
            cx.notify();
            return;
        }
        self.searching = true;
        cx.notify();
        let generation = self.search_generation;
        let url = self.config.search_url(&trimmed);
        self._search_task = Some(cx.spawn(async move |this, cx| {
            if !debounce.is_zero() {
                cx.background_executor().timer(debounce).await;
                if this
                    .update(cx, |this, _| this.search_generation != generation)
                    .unwrap_or(true)
                {
                    return;
                }
            }
            let results = fetch_gifs(&url).await;
            let _ = this.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                this.searching = false;
                match results {
                    Some(results) => this.search_results = results,
                    None => this.search_results.clear(),
                }
                cx.emit(GifEvent::Changed);
                cx.notify();
            });
        }));
    }

    pub fn categories(&self) -> &[GifCategory] {
        &self.categories
    }

    pub fn featured(&self) -> &[Gif] {
        &self.featured
    }

    pub fn search_results(&self) -> &[Gif] {
        &self.search_results
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn is_searching(&self) -> bool {
        self.searching
    }
}

async fn fetch_categories(url: &str) -> Option<Vec<GifCategory>> {
    let bytes = match mezon_client::transport_runtime::fetch_bytes(url).await {
        Ok((bytes, _)) => bytes,
        Err(_) => {
            tracing::error!("klipy categories fetch failed");
            return None;
        }
    };
    match serde_json::from_slice::<KlipyCategoriesResponse>(&bytes) {
        Ok(response) => Some(
            response
                .data
                .categories
                .into_iter()
                .filter_map(category_from_klipy)
                .collect(),
        ),
        Err(e) => {
            tracing::error!("klipy categories parse failed: {e}");
            None
        }
    }
}

async fn fetch_gifs(url: &str) -> Option<Vec<Gif>> {
    let bytes = match mezon_client::transport_runtime::fetch_bytes(url).await {
        Ok((bytes, _)) => bytes,
        Err(_) => {
            tracing::error!("klipy gifs fetch failed");
            return None;
        }
    };
    match serde_json::from_slice::<KlipyGifsResponse>(&bytes) {
        Ok(response) => Some(
            response
                .data
                .data
                .into_iter()
                .filter_map(gif_from_klipy)
                .collect(),
        ),
        Err(e) => {
            tracing::error!("klipy gifs parse failed: {e}");
            None
        }
    }
}

fn category_from_klipy(category: KlipyCategory) -> Option<GifCategory> {
    let name = category.category.unwrap_or_default();
    let query = category.query.unwrap_or_default();
    if name.is_empty() && query.is_empty() {
        return None;
    }
    let searchterm = if query.is_empty() {
        name.clone()
    } else {
        query
    };
    Some(GifCategory {
        name: name.into(),
        searchterm: searchterm.into(),
        image: category.preview_url.unwrap_or_default().into(),
    })
}

fn gif_from_klipy(gif: KlipyGif) -> Option<Gif> {
    let still = gif.file.shareable()?;
    let url: SharedString = still.url().to_owned().into();
    Some(Gif {
        preview_url: url.clone(),
        url,
        width: still.width.unwrap_or_default(),
        height: still.height.unwrap_or_default(),
    })
}

fn encode_query(query: &str) -> String {
    let mut encoded = String::with_capacity(query.len());
    for byte in query.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> KlipyConfig {
        KlipyConfig {
            key: "KEY".into(),
            base_url: "https://api.klipy.com/api/v1".into(),
        }
    }

    #[test]
    fn builds_klipy_urls_with_expected_params() {
        let config = config();
        assert_eq!(
            config.categories_url(),
            "https://api.klipy.com/api/v1/KEY/gifs/categories"
        );
        assert_eq!(
            config.featured_url(),
            "https://api.klipy.com/api/v1/KEY/gifs/trending?page=1&per_page=30&format_filter=gif"
        );
        assert_eq!(
            config.search_url("happy cat"),
            "https://api.klipy.com/api/v1/KEY/gifs/search?page=1&per_page=30&q=happy%20cat&format_filter=gif"
        );
    }

    #[test]
    fn trailing_slash_in_base_url_does_not_double_up() {
        let config = KlipyConfig {
            key: "KEY".into(),
            base_url: "https://api.klipy.com/api/v1/".into(),
        };
        assert_eq!(
            config.categories_url(),
            "https://api.klipy.com/api/v1/KEY/gifs/categories"
        );
    }

    #[test]
    fn encodes_reserved_query_characters() {
        assert_eq!(encode_query("a b&c"), "a%20b%26c");
        assert_eq!(encode_query("plain-text_1.0~"), "plain-text_1.0~");
    }

    #[test]
    fn parses_a_klipy_trending_payload() {
        let payload = br#"{
            "result": true,
            "data": {
                "current_page": 1,
                "per_page": 30,
                "data": [
                    {
                        "id": 1,
                        "slug": "naruto-sleep",
                        "type": "gif",
                        "file": {
                            "xs": {"gif": {"url": "https://static.klipy.com/xs.gif", "width": 160, "height": 90}},
                            "sm": {"gif": {"url": "https://static.klipy.com/sm.gif", "width": 220, "height": 124}}
                        }
                    },
                    {
                        "id": 2,
                        "slug": "no-xs-variant",
                        "type": "gif",
                        "file": {
                            "md": {"gif": {"url": "https://static.klipy.com/md.gif", "width": 320, "height": 180}}
                        }
                    },
                    { "id": 3, "slug": "no-gif-at-all", "type": "gif", "file": {} }
                ]
            }
        }"#;

        let response: KlipyGifsResponse = serde_json::from_slice(payload).expect("klipy shape");
        let gifs: Vec<Gif> = response
            .data
            .data
            .into_iter()
            .filter_map(gif_from_klipy)
            .collect();

        assert_eq!(gifs.len(), 2);
        assert_eq!(gifs[0].url.as_ref(), "https://static.klipy.com/xs.gif");
        assert_eq!(gifs[1].url.as_ref(), "https://static.klipy.com/md.gif");
    }

    #[test]
    fn sends_xs_gif_with_its_real_dimensions_like_react() {
        let json = r#"{"result":true,"data":{"data":[
            {"file":{
                "xs":{"gif":{"url":"https://static.klipy.com/a-xs.gif","width":213,"height":90}},
                "md":{"gif":{"url":"https://static.klipy.com/a-md.gif","width":500,"height":280}}
            }}
        ]}}"#;
        let response: KlipyGifsResponse = serde_json::from_str(json).unwrap();
        let gifs: Vec<Gif> = response
            .data
            .data
            .into_iter()
            .filter_map(gif_from_klipy)
            .collect();
        assert_eq!(gifs.len(), 1);
        assert_eq!(gifs[0].url, "https://static.klipy.com/a-xs.gif");
        assert_eq!(gifs[0].preview_url, "https://static.klipy.com/a-xs.gif");
        assert_eq!((gifs[0].width, gifs[0].height), (213, 90));
    }

    #[test]
    fn falls_back_upward_when_xs_missing_and_skips_items_without_a_gif() {
        let json = r#"{"result":true,"data":{"data":[
            {"file":{"sm":{"gif":{"url":"https://static.klipy.com/b-sm.gif","width":300,"height":200}}}},
            {"file":{"xs":{"gif":{"url":""}}}},
            {"file":{}}
        ]}}"#;
        let response: KlipyGifsResponse = serde_json::from_str(json).unwrap();
        let gifs: Vec<Gif> = response
            .data
            .data
            .into_iter()
            .filter_map(gif_from_klipy)
            .collect();
        assert_eq!(gifs.len(), 1);
        assert_eq!(gifs[0].url, "https://static.klipy.com/b-sm.gif");
        assert_eq!((gifs[0].width, gifs[0].height), (300, 200));
    }

    #[test]
    fn a_null_scalar_does_not_discard_the_whole_response() {
        let json = r#"{"result":true,"data":{"data":[
            {"file":{"xs":{"gif":{"url":null,"width":null,"height":null}},
                     "sm":{"gif":{"url":"https://static.klipy.com/ok.gif","width":220,"height":178}}}},
            {"file":{"xs":{"gif":{"url":"https://static.klipy.com/b.gif","width":null,"height":90}}}}
        ]}}"#;
        let response: KlipyGifsResponse = serde_json::from_str(json).unwrap();
        let gifs: Vec<Gif> = response
            .data
            .data
            .into_iter()
            .filter_map(gif_from_klipy)
            .collect();
        assert_eq!(gifs.len(), 2);
        assert_eq!(gifs[0].url, "https://static.klipy.com/ok.gif");
        assert_eq!((gifs[0].width, gifs[0].height), (220, 178));
        assert_eq!(gifs[1].url, "https://static.klipy.com/b.gif");
        assert_eq!((gifs[1].width, gifs[1].height), (0, 90));
    }

    #[test]
    fn a_null_category_field_does_not_discard_the_whole_response() {
        let json = r#"{"result":true,"data":{"categories":[
            {"category":"hello","query":null,"preview_url":null},
            {"category":null,"query":null,"preview_url":null}
        ]}}"#;
        let response: KlipyCategoriesResponse = serde_json::from_str(json).unwrap();
        let cats: Vec<GifCategory> = response
            .data
            .categories
            .into_iter()
            .filter_map(category_from_klipy)
            .collect();
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].name, "hello");
        assert_eq!(cats[0].searchterm, "hello");
    }

    #[test]
    fn parses_categories_and_skips_empty() {
        let json = r#"{"result":true,"data":{"locale":"en_US","categories":[
            {"category":"hello","query":"hello","preview_url":"https://static.klipy.com/hello.gif"},
            {"category":"","query":"","preview_url":""}
        ]}}"#;
        let response: KlipyCategoriesResponse = serde_json::from_str(json).unwrap();
        let categories: Vec<GifCategory> = response
            .data
            .categories
            .into_iter()
            .filter_map(category_from_klipy)
            .collect();
        assert_eq!(categories.len(), 1);
        assert_eq!(categories[0].name, "hello");
        assert_eq!(categories[0].searchterm, "hello");
        assert_eq!(categories[0].image, "https://static.klipy.com/hello.gif");
    }
}
