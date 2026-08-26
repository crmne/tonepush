//! The TonePush web API: Songs are musical ideas and Tones are playable,
//! device-native presets that belong to them.
//!
//! Public discovery is deliberately typed at this boundary. The library only
//! needs Song file hashes today, while the same client also exposes Song
//! details and individual Tone downloads for the browser/install UI. Publishing
//! mirrors the two resources on the server: create a Song, then add its first
//! Tone. If the second request fails the error says that the Song was created;
//! there is no delete call and therefore no pretend rollback.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::mpsc::{channel, Receiver};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::update::VERSION;

const SITE: &str = "https://tonepush.rocks";
const PAGES: u32 = 100;
const MAX_FEED_PAGES: usize = 1_000;

/// Where the web application lives. `TONEPUSH_SITE` is useful when exercising
/// the editor against a local Rails server.
pub fn site() -> String {
    std::env::var("TONEPUSH_SITE").unwrap_or_else(|_| SITE.to_owned())
}

fn agent_name() -> String {
    format!("TonePush {VERSION} ({})", std::env::consts::OS)
}

fn api_agent() -> ureq::Agent {
    ureq::config::Config::builder()
        .http_status_as_error(false)
        .build()
        .new_agent()
}

/// A catalog song by an Artist or an original musical idea.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SongKind {
    Song,
    Original,
}

/// One row returned by `GET /api/v1/songs` and embedded in Tone details.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SongSummary {
    pub id: i64,
    pub title: String,
    pub kind: SongKind,
    pub artist: Option<String>,
    pub part: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    pub tuning: Option<String>,
    pub guitar_type: Option<String>,
    pub pickup_type: Option<String>,
    pub pickup_electronics: Option<String>,
    pub tone_count: u64,
    #[serde(default)]
    pub devices: Vec<String>,
    #[serde(default)]
    pub file_sha256s: Vec<String>,
}

/// One Song together with the playable Tones a person may choose from.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SongDetails {
    #[serde(flatten)]
    pub summary: SongSummary,
    #[serde(default)]
    pub tones: Vec<ToneDetails>,
}

/// The device a Tone can be installed on.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DeviceSummary {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub family: Option<String>,
    pub manufacturer: String,
    #[serde(default)]
    pub capabilities: serde_json::Value,
    #[serde(default)]
    pub artifact_extensions: Vec<String>,
}

/// The fields needed to present a playable Tone in a Song detail view.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ToneSummary {
    pub id: i64,
    pub song_id: i64,
    pub name: String,
    pub device: DeviceSummary,
    pub creator: Option<String>,
    pub description: Option<String>,
    pub state: String,
    pub availability: String,
    pub source_kind: String,
    pub firmware_version: Option<String>,
    pub minimum_firmware_version: Option<String>,
    pub parser_version: Option<String>,
    pub installs_count: u64,
    pub saves_count: u64,
    pub remix_count: u64,
    /// Stable public identity supplied by the originating Editor library.
    #[serde(default)]
    pub series_id: Option<String>,
    /// The stable Tone id once it has more than one stored artifact revision.
    #[serde(default)]
    pub version_root_id: Option<i64>,
    /// One-based current revision within that stable Tone.
    #[serde(default)]
    pub version_number: Option<u32>,
    /// Total artifact revisions available for this Tone.
    #[serde(default)]
    pub versions_count: u32,
    /// Aggregate source rating when the upstream catalog provides one.
    pub rating: Option<f32>,
    /// When this preset first entered the public catalog.
    #[serde(default)]
    pub created_at: String,
    /// When its public record was last changed.
    #[serde(default)]
    pub updated_at: String,
    pub parent_id: Option<i64>,
    #[serde(default)]
    pub signal_chain: Vec<serde_json::Value>,
}

/// Where a playable Tone comes from. Native Tones have an artifact path;
/// externally indexed Tones lead to their original source instead.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ToneDownload {
    pub artifact: Option<String>,
    pub external: Option<String>,
}

/// The complete response from `GET /api/v1/tones/:id`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ToneDetails {
    #[serde(flatten)]
    pub summary: ToneSummary,
    pub file_sha256: Option<String>,
    #[serde(default)]
    pub parsed_metadata: serde_json::Value,
    #[serde(default)]
    pub dependencies: Vec<serde_json::Value>,
    #[serde(default)]
    pub audio_previews: Vec<serde_json::Value>,
    #[serde(default)]
    pub versions: Vec<ToneVersion>,
    pub download: Option<ToneDownload>,
    /// Present on the individual Tone endpoint. A Tone embedded in its own
    /// Song details does not repeat the parent Song.
    pub song: Option<SongSummary>,
}

/// One immutable public artifact revision nested beneath a stable Tone.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ToneVersion {
    pub number: u32,
    #[serde(default)]
    pub current: bool,
    pub file_sha256: String,
    #[serde(default)]
    pub created_at: String,
    pub download: ToneDownload,
}

/// One installable row in the desktop browser. The web API groups Tones under
/// Songs, while the editor's library is one row per playable preset, so the
/// parent travels with each result instead of being looked up again by the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredTone {
    pub song: SongSummary,
    pub tone: ToneDetails,
}

/// One page of the public Tone feed and the catalog-wide matching count.
///
/// The count lets the Editor label the Cloud tab after one request. `next_page`
/// is deliberately explicit: callers never have to crawl the catalog merely
/// to discover whether another page exists.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveryPage {
    pub entries: Vec<DiscoveredTone>,
    pub total: usize,
    pub next_page: Option<u32>,
}

/// Facets accepted by Song search. Empty values are not sent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SongSearch {
    pub q: Option<String>,
    pub artist: Option<String>,
    pub device: Option<String>,
    pub output_target: Option<String>,
    pub genre: Option<String>,
    pub sort: Option<String>,
    pub page: u32,
}

/// The useful catalog-wide orders offered by public discovery.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiscoveryOrder {
    #[default]
    Popular,
    Newest,
    Updated,
    Rating,
}

impl DiscoveryOrder {
    pub const ALL: [Self; 4] = [Self::Popular, Self::Newest, Self::Updated, Self::Rating];

    pub fn label(self) -> &'static str {
        match self {
            Self::Popular => "Most downloaded",
            Self::Newest => "Newest",
            Self::Updated => "Recently updated",
            Self::Rating => "Highest rated",
        }
    }

    fn api_value(self) -> &'static str {
        match self {
            Self::Popular => "popular",
            Self::Newest => "newest",
            Self::Updated => "updated",
            Self::Rating => "rating",
        }
    }
}

#[derive(Debug, Deserialize)]
struct SongsResponse {
    songs: Vec<SongSummary>,
}

#[derive(Debug, Deserialize)]
struct TonesResponse {
    tones: Vec<ToneDetails>,
    /// Present on catalog endpoints that can page the complete matching feed.
    /// Optional keeps the Editor compatible with an older TonePush deployment.
    #[serde(default)]
    total: Option<usize>,
}

/// A client rooted at one TonePush web deployment.
#[derive(Clone)]
pub struct CloudClient {
    base: String,
    http: ureq::Agent,
}

impl CloudClient {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_owned(),
            http: api_agent(),
        }
    }

    fn songs_url(&self) -> String {
        format!("{}/api/v1/songs", self.base)
    }

    fn tones_url(&self) -> String {
        format!("{}/api/v1/tones", self.base)
    }

    /// Search musical ideas. The returned rows are Songs, not installable
    /// presets; call [`Self::song`] and let the person choose one of its Tones.
    pub fn search_songs(&self, search: &SongSearch) -> Result<Vec<SongSummary>, String> {
        let mut request = self
            .http
            .get(self.songs_url())
            .header("User-Agent", agent_name())
            .header("Accept", "application/json")
            .query("page", search.page.max(1).to_string());
        for (key, value) in [
            ("q", search.q.as_deref()),
            ("artist", search.artist.as_deref()),
            ("device", search.device.as_deref()),
            ("output_target", search.output_target.as_deref()),
            ("genre", search.genre.as_deref()),
            ("sort", search.sort.as_deref()),
        ] {
            if let Some(value) = value.filter(|value| !value.is_empty()) {
                request = request.query(key, value);
            }
        }
        let response = request
            .call()
            .map_err(|error| format!("the Song catalog did not answer: {error}"))?;
        decode(response).map(|response: SongsResponse| response.songs)
    }

    /// Search the flat public Tone feed. `None` means this deployment predates
    /// that endpoint, in which case discovery falls back to expanding Songs.
    fn search_tones(&self, search: &SongSearch) -> Result<Option<TonesResponse>, String> {
        let mut request = self
            .http
            .get(self.tones_url())
            .header("User-Agent", agent_name())
            .header("Accept", "application/json")
            .query("page", search.page.max(1).to_string());
        for (key, value) in [
            ("q", search.q.as_deref()),
            ("artist", search.artist.as_deref()),
            ("device", search.device.as_deref()),
            ("output_target", search.output_target.as_deref()),
            ("genre", search.genre.as_deref()),
            ("sort", search.sort.as_deref()),
        ] {
            if let Some(value) = value.filter(|value| !value.is_empty()) {
                request = request.query(key, value);
            }
        }
        let response = request
            .call()
            .map_err(|error| format!("the Tone catalog did not answer: {error}"))?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        decode(response).map(Some)
    }

    /// Fetch named pages in at most eight lanes, preserving their page numbers
    /// so callers can put an out-of-order network answer back in catalog order.
    fn tone_pages(
        &self,
        search: &SongSearch,
        page_numbers: &[u32],
    ) -> Result<Vec<(u32, Vec<ToneDetails>)>, String> {
        const LANES: usize = 8;
        let lane_size = page_numbers.len().div_ceil(LANES).max(1);
        std::thread::scope(|scope| -> Result<Vec<_>, String> {
            let handles: Vec<_> = page_numbers
                .chunks(lane_size)
                .map(|lane| {
                    let client = self.clone();
                    let lane = lane.to_vec();
                    let search = search.clone();
                    scope.spawn(move || {
                        lane.into_iter()
                            .map(|page| {
                                let mut request = search.clone();
                                request.page = page;
                                let response = client.search_tones(&request)?.ok_or_else(|| {
                                    "the Tone catalog disappeared while paging".to_owned()
                                })?;
                                Ok((page, response.tones))
                            })
                            .collect::<Result<Vec<_>, String>>()
                    })
                })
                .collect();
            let mut found = Vec::new();
            for handle in handles {
                found.extend(
                    handle
                        .join()
                        .map_err(|_| "the Tone catalog search stopped".to_owned())??,
                );
            }
            found.sort_by_key(|(page, _)| *page);
            Ok(found)
        })
    }

    /// Fetch every page in the flat Tone feed. A current server tells us the
    /// total, so the remaining pages travel in a few bounded lanes; an older
    /// one is followed until it returns a short page.
    fn all_tones(&self, search: &SongSearch) -> Result<Option<TonesResponse>, String> {
        const PAGE_SIZE: usize = 50;
        const LANES: usize = 8;

        let Some(first) = self.search_tones(search)? else {
            return Ok(None);
        };
        let reported_total = first.total;
        let mut pages = vec![(1_u32, first.tones)];

        if let Some(total) = reported_total {
            let last = total.div_ceil(PAGE_SIZE);
            if last > MAX_FEED_PAGES {
                return Err(format!(
                    "the Tone catalog reported {total} matches, more than can be fetched safely"
                ));
            }
            let last = last as u32;
            let remaining: Vec<u32> = (2..=last).collect();
            pages.extend(self.tone_pages(search, &remaining)?);
        } else {
            let mut page = 2_u32;
            while page <= MAX_FEED_PAGES as u32
                && pages
                    .last()
                    .is_some_and(|(_, tones)| tones.len() == PAGE_SIZE)
            {
                let batch: Vec<u32> =
                    (page..(page + LANES as u32).min(MAX_FEED_PAGES as u32 + 1)).collect();
                let fetched = self.tone_pages(search, &batch)?;
                let end = fetched
                    .iter()
                    .find(|(_, tones)| tones.len() < PAGE_SIZE)
                    .map(|(page, _)| *page);
                pages.extend(
                    fetched
                        .into_iter()
                        .filter(|(page, _)| end.is_none_or(|end| *page <= end)),
                );
                if end.is_some() {
                    break;
                }
                page += LANES as u32;
            }
        }

        pages.sort_by_key(|(page, _)| *page);
        Ok(Some(TonesResponse {
            tones: pages.into_iter().flat_map(|(_, tones)| tones).collect(),
            total: reported_total,
        }))
    }

    /// Fetch exactly one current flat-feed page. The server's matching total
    /// arrives with every page, so the first request can label the whole feed
    /// without downloading it. An older deployment falls back to the complete
    /// compatibility search once; it has no total or stable paging contract.
    pub fn discover_page(
        &self,
        query: &str,
        order: DiscoveryOrder,
        device: Option<&str>,
        page: u32,
    ) -> Result<DiscoveryPage, String> {
        const PAGE_SIZE: usize = 50;

        let search = SongSearch {
            q: (!query.trim().is_empty()).then(|| query.trim().to_owned()),
            device: device.map(catalog_device_filter),
            sort: Some(order.api_value().to_owned()),
            page: page.max(1),
            ..Default::default()
        };
        let response = match self.search_tones(&search)? {
            Some(response) if response.total.is_some() => response,
            // Compatibility behavior remains centralized in `discover`: old
            // servers need unfiltered/local filtering and Song expansion.
            _ if page <= 1 => {
                let entries = self.discover(query, order, device)?;
                return Ok(DiscoveryPage {
                    total: entries.len(),
                    entries,
                    next_page: None,
                });
            }
            _ => return Err("the Tone catalog no longer supports paging".to_owned()),
        };
        let total = response.total.unwrap_or(response.tones.len());
        let entries = discovered_tones(response.tones, device)?;
        let page = page.max(1);
        Ok(DiscoveryPage {
            entries,
            total,
            next_page: ((u128::from(page) * PAGE_SIZE as u128) < total as u128)
                .then(|| page.checked_add(1))
                .flatten(),
        })
    }

    /// Fetch one musical idea and the playable Tones belonging to it.
    pub fn song(&self, id: i64) -> Result<SongDetails, String> {
        let response = self
            .http
            .get(format!("{}/api/v1/songs/{id}", self.base))
            .header("User-Agent", agent_name())
            .header("Accept", "application/json")
            .call()
            .map_err(|error| format!("the Song catalog did not answer: {error}"))?;
        decode(response)
    }

    /// Fetch one playable, device-native Tone.
    pub fn tone(&self, id: i64) -> Result<ToneDetails, String> {
        let response = self
            .http
            .get(format!("{}/api/v1/tones/{id}", self.base))
            .header("User-Agent", agent_name())
            .header("Accept", "application/json")
            .call()
            .map_err(|error| format!("the Tone catalog did not answer: {error}"))?;
        decode(response)
    }

    /// Search every page of the public catalog for playable Tone rows. With a
    /// connected device the server narrows the feed to compatible formats;
    /// the same check is repeated locally so an older server cannot leak an
    /// incompatible row into the badge or table.
    pub fn discover(
        &self,
        query: &str,
        order: DiscoveryOrder,
        device: Option<&str>,
    ) -> Result<Vec<DiscoveredTone>, String> {
        let mut search = SongSearch {
            q: (!query.trim().is_empty()).then(|| query.trim().to_owned()),
            device: device.map(catalog_device_filter),
            sort: Some(order.api_value().to_owned()),
            page: 1,
            ..Default::default()
        };
        let flat = self.all_tones(&search);
        let flat = match flat {
            // A response without a total predates server-side product-name
            // filtering. Fetch its unfiltered pages and narrow them here.
            Ok(Some(response)) if device.is_some() && response.total.is_none() => {
                search.device = None;
                self.all_tones(&search)?
            }
            Ok(response) => response,
            // Older deployments treated a product name as a numeric id and
            // could reject it. The public unfiltered feed remains usable.
            Err(_) if device.is_some() => {
                search.device = None;
                self.all_tones(&search)?
            }
            Err(why) => return Err(why),
        };
        if let Some(response) = flat {
            return response
                .tones
                .into_iter()
                .filter(|tone| {
                    device.is_none_or(|connected| {
                        compatible_device(connected, &tone.summary.device.name)
                    })
                })
                .map(|tone| {
                    let song = tone
                        .song
                        .clone()
                        .ok_or_else(|| format!("{} arrived without its Song", tone.summary.name))?;
                    Ok(DiscoveredTone { song, tone })
                })
                .collect();
        }

        // Compatibility with a server from before the flat feed: expand its
        // Song page exactly as the first desktop browser did.
        // The legacy Song endpoint accepted a database id, not a product
        // name. Expand it unfiltered, then apply compatibility below.
        search.device = None;
        let songs = self.search_songs(&search)?;
        // A search page can hold dozens of Songs and the index intentionally
        // does not embed their Tones. Expand it in a few bounded lanes rather
        // than making discovery wait on dozens of serial round trips.
        let lane_size = songs.len().div_ceil(8).max(1);
        let fetched = std::thread::scope(|scope| -> Result<Vec<_>, String> {
            let handles: Vec<_> = songs
                .chunks(lane_size)
                .map(|lane| {
                    let client = self.clone();
                    scope.spawn(move || {
                        lane.iter()
                            .map(|summary| client.song(summary.id))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            let mut fetched = Vec::new();
            for handle in handles {
                fetched.extend(
                    handle
                        .join()
                        .map_err(|_| "the Song catalog search stopped".to_owned())?,
                );
            }
            Ok(fetched)
        })?;
        let mut found = Vec::new();
        let mut first_error = None;
        for details in fetched {
            let details = match details {
                Ok(details) => details,
                Err(why) => {
                    first_error.get_or_insert(why);
                    continue;
                }
            };
            found.extend(
                details
                    .tones
                    .into_iter()
                    .filter(|tone| {
                        device.is_none_or(|connected| {
                            compatible_device(connected, &tone.summary.device.name)
                        })
                    })
                    .map(|tone| DiscoveredTone {
                        song: details.summary.clone(),
                        tone,
                    }),
            );
        }
        if found.is_empty() {
            if let Some(why) = first_error {
                return Err(why);
            }
        }
        Ok(found)
    }

    /// Resolve a Tone's download without pretending an external catalog entry
    /// is a native artifact. Callers open [`ToneDelivery::External`] in the
    /// browser and install [`ToneDelivery::Artifact`] into the local library.
    pub fn download(&self, tone: &ToneDetails) -> Result<ToneDelivery, String> {
        let Some(location) = &tone.download else {
            return Err(format!("{} has no downloadable preset", tone.summary.name));
        };
        if let Some(url) = &location.external {
            return Ok(ToneDelivery::External(url.clone()));
        }
        let Some(path) = &location.artifact else {
            return Err(format!("{} has no downloadable preset", tone.summary.name));
        };
        let url = if path.starts_with("http://") || path.starts_with("https://") {
            path.clone()
        } else {
            format!("{}/{}", self.base, path.trim_start_matches('/'))
        };
        let mut response = self
            .http
            .get(url)
            .header("User-Agent", agent_name())
            .call()
            .map_err(|error| format!("the Tone artifact did not answer: {error}"))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let body = response.body_mut().read_to_string().unwrap_or_default();
            return Err(api_error(status, &body));
        }
        let bytes = response
            .body_mut()
            .read_to_vec()
            .map_err(|error| format!("the Tone artifact was unreadable: {error}"))?;
        Ok(ToneDelivery::Artifact(bytes))
    }

    /// Create a musical idea. This operation never uploads a preset.
    pub fn create_song(
        &self,
        token: &str,
        request: &CreateSongRequest,
    ) -> Result<SongDetails, String> {
        let body = serde_json::to_vec(request)
            .map_err(|error| format!("the Song could not be encoded: {error}"))?;
        let response = self
            .http
            .post(self.songs_url())
            .header("User-Agent", agent_name())
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {token}"))
            .content_type("application/json")
            .send(body)
            .map_err(|error| format!("the Song catalog did not answer: {error}"))?;
        decode(response)
    }

    /// Add one playable Tone to an existing Song.
    pub fn create_tone(
        &self,
        token: &str,
        song_id: i64,
        request: &CreateToneRequest,
    ) -> Result<ToneDetails, String> {
        let form = request.multipart();
        let response = self
            .http
            .post(format!("{}/api/v1/songs/{song_id}/tones", self.base))
            .header("User-Agent", agent_name())
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", form.content_type())
            .send(form.finish())
            .map_err(|error| format!("the Tone catalog did not answer: {error}"))?;
        decode(response)
    }

    /// Publish through the resourceful two-step contract. An existing Song
    /// goes straight to Tone creation; a new one is created first.
    pub fn publish(
        &self,
        token: &str,
        request: &PublishRequest,
    ) -> Result<ToneDetails, PublishError> {
        let (song_id, created) = match &request.song {
            PublishSong::Existing(id) => (*id, None),
            PublishSong::New(song) => {
                let created = self
                    .create_song(token, song)
                    .map_err(PublishError::CreatingSong)?;
                (created.summary.id, Some(created.summary.title))
            }
        };
        self.create_tone(token, song_id, &request.tone)
            .map_err(|reason| PublishError::CreatingTone {
                song_id,
                created_song: created,
                reason,
            })
    }
}

fn discovered_tones(
    tones: Vec<ToneDetails>,
    device: Option<&str>,
) -> Result<Vec<DiscoveredTone>, String> {
    tones
        .into_iter()
        .filter(|tone| {
            device.is_none_or(|connected| compatible_device(connected, &tone.summary.device.name))
        })
        .map(|tone| {
            let song = tone
                .song
                .clone()
                .ok_or_else(|| format!("{} arrived without its Song", tone.summary.name))?;
            Ok(DiscoveredTone { song, tone })
        })
        .collect()
}

/// Bridge the hardware profile names and the public catalog's product names.
/// The XL reads Stomp presets and the web catalog calls a Helix Floor simply
/// “Helix”; every other pairing remains exact and conservative.
pub fn compatible_device(connected: &str, published: &str) -> bool {
    connected == published
        || matches!(
            (connected, published),
            ("HX Stomp XL", "HX Stomp") | ("Helix Floor", "Helix")
        )
}

/// The product selectors understood by the catalog API for one connected
/// device. Compatibility aliases live beside the local check above so the two
/// cannot silently drift apart.
fn catalog_device_filter(connected: &str) -> String {
    match connected {
        "HX Stomp XL" => "HX Stomp XL,HX Stomp",
        "Helix Floor" => "Helix Floor,Helix",
        other => other,
    }
    .to_owned()
}

/// A resolved Tone download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToneDelivery {
    Artifact(Vec<u8>),
    External(String),
}

/// Search the configured TonePush deployment for desktop-library rows.
pub fn discover(
    query: &str,
    order: DiscoveryOrder,
    device: Option<&str>,
) -> Result<Vec<DiscoveredTone>, String> {
    CloudClient::new(site()).discover(query, order, device)
}

/// Fetch one configured TonePush discovery page.
pub fn discover_page(
    query: &str,
    order: DiscoveryOrder,
    device: Option<&str>,
    page: u32,
) -> Result<DiscoveryPage, String> {
    CloudClient::new(site()).discover_page(query, order, device, page)
}

/// Fetch one discovered Tone from the configured TonePush deployment.
pub fn download(tone: &ToneDetails) -> Result<ToneDelivery, String> {
    CloudClient::new(site()).download(tone)
}

/// The JSON body for creating one Song.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateSongRequest {
    pub creator_name: String,
    pub song: NewSong,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewSong {
    pub title: String,
    pub kind: SongKind,
    pub artist_name: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub genre_ids: Vec<i64>,
}

/// The portable preset attached to a new Tone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetUpload {
    pub filename: String,
    pub bytes: Vec<u8>,
}

/// The multipart body for adding a Tone to a Song.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreateToneRequest {
    pub creator_name: String,
    pub tone: NewTone,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewTone {
    pub name: String,
    /// Stable local tone identity, used by the server to revise rather than
    /// duplicate a previously published Tone.
    pub series_id: Option<String>,
    pub description: Option<String>,
    pub part: Option<String>,
    pub tuning: Option<String>,
    pub guitar_type: Option<String>,
    pub pickup_type: Option<String>,
    pub pickup_electronics: Option<String>,
    pub device_id: Option<i64>,
    pub device_name: Option<String>,
    pub firmware_version: Option<String>,
    pub parser_version: Option<String>,
    pub output_target: Option<String>,
    pub chain_content: Option<String>,
    pub character: Option<String>,
    pub blocks: Vec<serde_json::Value>,
    pub parsed_metadata: serde_json::Value,
    pub preset: Option<PresetUpload>,
}

impl CreateToneRequest {
    fn multipart(&self) -> Multipart {
        let mut form = Multipart::new();
        if !self.creator_name.is_empty() {
            form.field("creator_name", &self.creator_name);
        }
        form.field("tone[name]", &self.tone.name);
        if let Some(series) = &self.tone.series_id {
            form.field("tone[series_id]", series);
        }
        for (field, value) in [
            ("description", self.tone.description.as_deref()),
            ("part", self.tone.part.as_deref()),
            ("tuning", self.tone.tuning.as_deref()),
            ("guitar_type", self.tone.guitar_type.as_deref()),
            ("pickup_type", self.tone.pickup_type.as_deref()),
            (
                "pickup_electronics",
                self.tone.pickup_electronics.as_deref(),
            ),
            ("device_name", self.tone.device_name.as_deref()),
            ("firmware_version", self.tone.firmware_version.as_deref()),
            ("parser_version", self.tone.parser_version.as_deref()),
            ("output_target", self.tone.output_target.as_deref()),
            ("chain_content", self.tone.chain_content.as_deref()),
            ("character", self.tone.character.as_deref()),
        ] {
            if let Some(value) = value {
                form.field(&format!("tone[{field}]"), value);
            }
        }
        if let Some(id) = self.tone.device_id {
            form.field("tone[device_id]", &id.to_string());
        }
        form.object_array("tone[blocks]", &self.tone.blocks);
        if !self.tone.parsed_metadata.is_null()
            && self
                .tone
                .parsed_metadata
                .as_object()
                .is_none_or(|object| !object.is_empty())
        {
            form.json("tone[parsed_metadata]", &self.tone.parsed_metadata);
        }
        if let Some(preset) = &self.tone.preset {
            form.file("tone[preset]", &preset.filename, &preset.bytes);
        }
        form
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PublishRequest {
    pub song: PublishSong,
    pub tone: CreateToneRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishSong {
    New(CreateSongRequest),
    Existing(i64),
}

/// Which of the two publishing operations failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishError {
    CreatingSong(String),
    CreatingTone {
        song_id: i64,
        created_song: Option<String>,
        reason: String,
    },
}

impl fmt::Display for PublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreatingSong(reason) => write!(formatter, "the Song was not created: {reason}"),
            Self::CreatingTone {
                song_id,
                created_song: Some(title),
                reason,
            } => write!(
                formatter,
                "Song “{title}” was created as #{song_id}, but its Tone was not published: \
                 {reason}. The empty Song remains on TonePush"
            ),
            Self::CreatingTone {
                song_id,
                created_song: None,
                reason,
            } => write!(
                formatter,
                "the Tone was not added to Song #{song_id}: {reason}"
            ),
        }
    }
}

/// Ask the Song index what native files are already published, off the UI
/// thread. Failure yields no value; a successful empty catalog yields an empty
/// set so the library can still offer its first publish action.
pub fn published() -> Receiver<BTreeSet<String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        if let Ok(hashes) = fetch_published(&CloudClient::new(site())) {
            let _ = tx.send(hashes);
        }
    });
    rx
}

fn fetch_published(client: &CloudClient) -> Result<BTreeSet<String>, String> {
    let mut found = BTreeSet::new();
    for page in 1..=PAGES {
        let songs = client.search_songs(&SongSearch {
            page,
            ..Default::default()
        })?;
        if songs.is_empty() {
            break;
        }
        for song in songs {
            found.extend(
                song.file_sha256s
                    .into_iter()
                    .filter(|hash| hash.len() == 64)
                    .map(|hash| hash.to_ascii_lowercase()),
            );
        }
    }
    Ok(found)
}

/// Open the published Tone represented by a portable preset hash. The web app
/// resolves the hash and redirects to the Tone's canonical Song-nested page,
/// so the editor never has to guess Artist or Song slugs.
pub fn tone_url(file_sha256: &str) -> String {
    format!(
        "{}/tones/files/{}",
        site(),
        file_sha256.trim().to_ascii_lowercase()
    )
}

/// Signing this computer in remains the existing pairing flow.
fn pairings() -> String {
    format!("{}/api/v1/pairings", site())
}

#[derive(Debug, Clone)]
pub struct Pairing {
    pub code: String,
    pub url: String,
}

pub enum Linked {
    In { token: String, account: String },
    GaveUp(String),
}

pub fn start_pairing() -> Result<Pairing, String> {
    let mut response = ureq::post(pairings())
        .header("User-Agent", agent_name())
        .header("Accept", "application/json")
        .send_empty()
        .map_err(|error| format!("TonePush did not answer the pairing request: {error}"))?;
    let body: serde_json::Value = decode_body(&mut response)?;
    let (Some(code), Some(url)) = (
        body.get("code").and_then(|code| code.as_str()),
        body.get("url").and_then(|url| url.as_str()),
    ) else {
        return Err("TonePush did not offer a pairing code".to_owned());
    };
    Ok(Pairing {
        code: code.to_owned(),
        url: url.to_owned(),
    })
}

pub fn poll_pairing(code: &str) -> Result<Option<Linked>, String> {
    let mut response = ureq::get(format!("{}/{code}", pairings()))
        .header("User-Agent", agent_name())
        .header("Accept", "application/json")
        .call()
        .map_err(|error| format!("TonePush stopped answering the pairing request: {error}"))?;
    let body: serde_json::Value = decode_body(&mut response)?;
    match body.get("state").and_then(|state| state.as_str()) {
        Some("linked") => {
            let token = body
                .get("token")
                .and_then(|token| token.as_str())
                .ok_or("TonePush paired without returning a session token")?;
            let account = body
                .get("name")
                .and_then(|name| name.as_str())
                .unwrap_or("your account");
            Ok(Some(Linked::In {
                token: token.to_owned(),
                account: account.to_owned(),
            }))
        }
        Some("pending") => Ok(None),
        _ => Ok(Some(Linked::GaveUp(
            "that code expired. Ask for a fresh one".to_owned(),
        ))),
    }
}

/// Publish a new Song and Tone against the configured site.
pub fn publish(token: &str, request: &PublishRequest) -> Result<ToneDetails, PublishError> {
    CloudClient::new(site()).publish(token, request)
}

fn decode<T: DeserializeOwned>(
    mut response: ureq::http::Response<ureq::Body>,
) -> Result<T, String> {
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|error| format!("TonePush answered nothing readable: {error}"))?;
    if !(200..300).contains(&status) {
        return Err(api_error(status, &body));
    }
    serde_json::from_str(&body)
        .map_err(|error| format!("TonePush answered with invalid JSON: {error}"))
}

fn decode_body<T: DeserializeOwned>(
    response: &mut ureq::http::Response<ureq::Body>,
) -> Result<T, String> {
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|error| format!("TonePush answered nothing readable: {error}"))?;
    serde_json::from_str(&body)
        .map_err(|error| format!("TonePush answered with invalid JSON: {error}"))
}

fn api_error(status: u16, body: &str) -> String {
    let parsed = serde_json::from_str::<serde_json::Value>(body).ok();
    let errors = parsed
        .as_ref()
        .and_then(|body| body.get("errors"))
        .and_then(|errors| errors.as_array())
        .into_iter()
        .flatten()
        .filter_map(|error| error.as_str())
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return errors.join("; ");
    }
    parsed
        .as_ref()
        .and_then(|body| body.get("error").or_else(|| body.get("message")))
        .and_then(|message| message.as_str())
        .filter(|message| !message.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("TonePush refused the request (HTTP {status})"))
}

/// A small multipart encoder with Rails-style nested field names.
struct Multipart {
    boundary: String,
    body: Vec<u8>,
}

impl Multipart {
    fn new() -> Self {
        let boundary = format!(
            "----TonePush{:x}",
            std::process::id() as u64 * 2_654_435_761
        );
        Self {
            boundary,
            body: Vec::new(),
        }
    }

    fn field(&mut self, name: &str, value: &str) {
        self.body.extend_from_slice(
            format!(
                "--{}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n",
                self.boundary
            )
            .as_bytes(),
        );
    }

    fn file(&mut self, name: &str, filename: &str, bytes: &[u8]) {
        self.body.extend_from_slice(
            format!(
                "--{}\r\nContent-Disposition: form-data; name=\"{name}\"; \
                 filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n",
                self.boundary
            )
            .as_bytes(),
        );
        self.body.extend_from_slice(bytes);
        self.body.extend_from_slice(b"\r\n");
    }

    fn object_array(&mut self, name: &str, values: &[serde_json::Value]) {
        for value in values {
            if let Some(object) = value.as_object() {
                for (key, value) in object {
                    self.json(&format!("{name}[][{key}]"), value);
                }
            }
        }
    }

    fn json(&mut self, name: &str, value: &serde_json::Value) {
        match value {
            serde_json::Value::Null => {}
            serde_json::Value::Bool(value) => self.field(name, &value.to_string()),
            serde_json::Value::Number(value) => self.field(name, &value.to_string()),
            serde_json::Value::String(value) => self.field(name, value),
            serde_json::Value::Array(values) => {
                for value in values {
                    self.json(&format!("{name}[]"), value);
                }
            }
            serde_json::Value::Object(values) => {
                for (key, value) in values {
                    self.json(&format!("{name}[{key}]"), value);
                }
            }
        }
    }

    fn content_type(&self) -> String {
        format!("multipart/form-data; boundary={}", self.boundary)
    }

    fn finish(mut self) -> Vec<u8> {
        self.body
            .extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        self.body
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    fn song_json(id: i64, title: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "title": title,
            "kind": "original",
            "artist": null,
            "part": "Clean",
            "description": "Wide and clean",
            "tags": ["clean"],
            "genres": ["Ambient"],
            "tuning": "Standard",
            "guitar_type": "Stratocaster",
            "pickup_type": "single_coil",
            "pickup_electronics": "passive",
            "tone_count": 1,
            "devices": ["HX Stomp"],
            "file_sha256s": ["A".repeat(64)]
        })
    }

    fn tone_json(id: i64, song_id: i64, title: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "song_id": song_id,
            "name": "Wide HX",
            "device": {
                "id": 1,
                "name": "HX Stomp",
                "slug": "hx-stomp",
                "family": "Helix",
                "manufacturer": "Line 6",
                "capabilities": {},
                "artifact_extensions": ["hlx"]
            },
            "creator": "Public Name",
            "description": "The playable preset",
            "state": "published",
            "availability": "free",
            "source_kind": "native",
            "firmware_version": "3.80",
            "minimum_firmware_version": null,
            "parser_version": "0.4.3",
            "installs_count": 0,
            "saves_count": 0,
            "remix_count": 0,
            "parent_id": null,
            "signal_chain": [{"name": "Minotaur"}],
            "file_sha256": "b".repeat(64),
            "parsed_metadata": {},
            "dependencies": [],
            "audio_previews": [],
            "download": {"artifact": format!("/tones/{id}/artifact")},
            "song": song_json(song_id, title)
        })
    }

    struct StubServer {
        base: String,
        requests: Arc<Mutex<Vec<Vec<u8>>>>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl StubServer {
        fn start(responses: Vec<(u16, serde_json::Value)>) -> Self {
            Self::start_raw(
                responses
                    .into_iter()
                    .map(|(status, body)| (status, serde_json::to_vec(&body).unwrap()))
                    .collect(),
            )
        }

        fn start_raw(responses: Vec<(u16, Vec<u8>)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let captured = requests.clone();
            let thread = std::thread::spawn(move || {
                for (status, body) in responses {
                    let (mut stream, _) = listener.accept().unwrap();
                    captured.lock().unwrap().push(read_request(&mut stream));
                    let reason = if (200..300).contains(&status) {
                        "OK"
                    } else {
                        "Unprocessable Entity"
                    };
                    write!(
                        stream,
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .unwrap();
                    stream.write_all(&body).unwrap();
                }
            });
            Self {
                base,
                requests,
                thread: Some(thread),
            }
        }

        fn finish(mut self) -> Vec<Vec<u8>> {
            self.thread.take().unwrap().join().unwrap();
            Arc::try_unwrap(self.requests)
                .unwrap()
                .into_inner()
                .unwrap()
        }
    }

    fn read_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let read = stream.read(&mut chunk).unwrap();
            assert!(read > 0);
            request.extend_from_slice(&chunk[..read]);
            if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break end + 4;
            }
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        while request.len() < header_end + length {
            let read = stream.read(&mut chunk).unwrap();
            assert!(read > 0);
            request.extend_from_slice(&chunk[..read]);
        }
        request
    }

    fn new_song() -> CreateSongRequest {
        CreateSongRequest {
            creator_name: "Public Name".into(),
            song: NewSong {
                title: "Wide Clean".into(),
                kind: SongKind::Original,
                artist_name: None,
                description: Some("A musical idea".into()),
                tags: vec!["clean".into(), "wide".into()],
                genre_ids: Vec::new(),
            },
        }
    }

    fn new_tone() -> CreateToneRequest {
        CreateToneRequest {
            creator_name: "Public Name".into(),
            tone: NewTone {
                name: "Wide HX".into(),
                series_id: Some("local-series".into()),
                part: Some("Clean".into()),
                device_name: Some("HX Stomp".into()),
                firmware_version: Some("3.80".into()),
                parser_version: Some(crate::update::VERSION.into()),
                blocks: vec![serde_json::json!({
                    "name": "Minotaur", "category": 1, "enabled": true, "path": 0
                })],
                parsed_metadata: serde_json::json!({"models_used": [101]}),
                preset: Some(PresetUpload {
                    filename: "wide.hlx".into(),
                    bytes: b"preset bytes".to_vec(),
                }),
                ..Default::default()
            },
        }
    }

    #[test]
    fn song_and_tone_wire_shapes_decode_separately() {
        let index: SongsResponse =
            serde_json::from_str(include_str!("../tests/fixtures/cloud/songs-index.json")).unwrap();
        let song: SongDetails =
            serde_json::from_str(include_str!("../tests/fixtures/cloud/song-details.json"))
                .unwrap();
        assert_eq!(index.songs[0], song.summary);
        assert_eq!(song.tones.len(), 2, "the UI must offer both playable Tones");
        assert_eq!(song.tones[0].summary.name, "Numb HX");
        assert!(song.tones[0]
            .download
            .as_ref()
            .and_then(|download| download.artifact.as_ref())
            .is_some());

        let tone: ToneDetails =
            serde_json::from_str(include_str!("../tests/fixtures/cloud/tone-details.json"))
                .unwrap();
        assert_eq!(tone.summary.song_id, song.summary.id);
        assert_eq!(
            tone.song.as_ref().map(|song| song.title.as_str()),
            Some(song.summary.title.as_str())
        );
        assert_eq!(tone.file_sha256, Some("a".repeat(64)));
    }

    #[test]
    fn song_index_hashes_are_normalized_for_library_matching() {
        let server = StubServer::start(vec![
            (
                200,
                serde_json::json!({"songs": [song_json(12, "Wide Clean")]}),
            ),
            (200, serde_json::json!({"songs": []})),
        ]);
        let client = CloudClient::new(&server.base);
        assert_eq!(
            fetch_published(&client).unwrap(),
            BTreeSet::from(["a".repeat(64)])
        );
        let requests = server.finish();
        assert!(String::from_utf8_lossy(&requests[0]).starts_with("GET /api/v1/songs?page=1 "));
    }

    #[test]
    fn search_sends_every_supported_song_facet() {
        let server = StubServer::start(vec![(200, serde_json::json!({"songs": []}))]);
        let client = CloudClient::new(&server.base);
        client
            .search_songs(&SongSearch {
                q: Some("wide clean".into()),
                artist: Some("7".into()),
                device: Some("2".into()),
                output_target: Some("frfr_pa".into()),
                genre: Some("3".into()),
                sort: Some("rating".into()),
                page: 4,
            })
            .unwrap();
        let request = server.finish().pop().unwrap();
        let request = String::from_utf8_lossy(&request);
        assert!(request.starts_with("GET /api/v1/songs?"));
        for pair in [
            "q=wide%20clean",
            "artist=7",
            "device=2",
            "output_target=frfr_pa",
            "genre=3",
            "sort=rating",
            "page=4",
        ] {
            assert!(request.contains(pair), "missing {pair} in {request}");
        }
    }

    #[test]
    fn discovery_without_a_known_device_keeps_every_catalog_format() {
        let mut song: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/cloud/song-details.json"))
                .unwrap();
        let tones = song
            .as_object_mut()
            .unwrap()
            .remove("tones")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .map(|mut tone| {
                tone["song"] = song.clone();
                tone
            })
            .collect::<Vec<_>>();
        let server = StubServer::start(vec![(200, serde_json::json!({"tones": tones}))]);
        let client = CloudClient::new(&server.base);

        let found = client
            .discover("comfortably numb", DiscoveryOrder::Popular, None)
            .unwrap();

        assert_eq!(
            found.len(),
            2,
            "both published device variants are discoverable"
        );
        assert_eq!(found[0].song.title, "Comfortably Numb");
        assert_eq!(found[0].tone.summary.name, "Numb HX");
        let requests = server.finish();
        let search = String::from_utf8_lossy(&requests[0]);
        assert!(search.starts_with("GET /api/v1/tones?"));
        assert!(search.contains("q=comfortably%20numb"));
        assert!(search.contains("sort=popular"));
        assert!(!search.contains("device="));
    }

    #[test]
    fn discovery_requests_and_keeps_only_compatible_device_formats() {
        let mut song: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/cloud/song-details.json"))
                .unwrap();
        let tones = song
            .as_object_mut()
            .unwrap()
            .remove("tones")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .map(|mut tone| {
                tone["song"] = song.clone();
                tone
            })
            .collect::<Vec<_>>();
        let server =
            StubServer::start(vec![(200, serde_json::json!({"tones": tones, "total": 2}))]);
        let client = CloudClient::new(&server.base);

        let found = client
            .discover(
                "comfortably numb",
                DiscoveryOrder::Popular,
                Some("HX Stomp"),
            )
            .unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].tone.summary.device.name, "HX Stomp");
        let request = server.finish().pop().unwrap();
        assert!(String::from_utf8_lossy(&request).contains("device=HX%20Stomp"));
    }

    #[test]
    fn discovery_fetches_every_flat_feed_page() {
        let first = (1..=50)
            .map(|id| tone_json(id, 12, "Wide Clean"))
            .collect::<Vec<_>>();
        let server = StubServer::start(vec![
            (200, serde_json::json!({"tones": first, "total": 51})),
            (
                200,
                serde_json::json!({"tones": [tone_json(51, 12, "Wide Clean")], "total": 51}),
            ),
        ]);
        let client = CloudClient::new(&server.base);

        let found = client.discover("", DiscoveryOrder::Popular, None).unwrap();

        assert_eq!(found.len(), 51);
        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        assert!(String::from_utf8_lossy(&requests[1]).contains("page=2"));
    }

    #[test]
    fn discovery_page_returns_the_total_without_fetching_the_next_page() {
        let first = (1..=50)
            .map(|id| tone_json(id, 12, "Wide Clean"))
            .collect::<Vec<_>>();
        let server = StubServer::start(vec![(
            200,
            serde_json::json!({"tones": first, "total": 51}),
        )]);
        let client = CloudClient::new(&server.base);

        let page = client
            .discover_page("", DiscoveryOrder::Popular, None, 1)
            .unwrap();

        assert_eq!(page.entries.len(), 50);
        assert_eq!(page.entries[0].tone.summary.id, 1);
        assert_eq!(page.total, 51);
        assert_eq!(page.next_page, Some(2));
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(String::from_utf8_lossy(&requests[0]).contains("page=1"));
    }

    #[test]
    fn impossible_catalog_pagination_is_bounded_without_integer_wrap() {
        let server = StubServer::start(vec![(
            200,
            serde_json::json!({"tones": [], "total": MAX_FEED_PAGES * 50 + 1}),
        )]);
        let client = CloudClient::new(&server.base);
        let search = SongSearch {
            page: 1,
            ..Default::default()
        };
        assert!(client.all_tones(&search).unwrap_err().contains("safely"));
        assert_eq!(server.finish().len(), 1, "no huge page fan-out");

        let server = StubServer::start(vec![(
            200,
            serde_json::json!({"tones": [], "total": usize::MAX}),
        )]);
        let client = CloudClient::new(&server.base);
        let page = client
            .discover_page("", DiscoveryOrder::Popular, None, u32::MAX)
            .unwrap();
        assert_eq!(page.next_page, None, "u32::MAX must not wrap to page zero");
        assert_eq!(server.finish().len(), 1);
    }

    #[test]
    fn discovery_falls_back_to_song_expansion_on_an_older_server() {
        let songs: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/cloud/songs-index.json")).unwrap();
        let song: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/cloud/song-details.json"))
                .unwrap();
        let server = StubServer::start(vec![
            (404, serde_json::json!({"error": "not found"})),
            (200, songs),
            (200, song),
        ]);
        let client = CloudClient::new(&server.base);

        let found = client
            .discover("comfortably numb", DiscoveryOrder::Popular, None)
            .unwrap();

        assert_eq!(found.len(), 2);
        let requests = server.finish();
        assert!(String::from_utf8_lossy(&requests[0]).starts_with("GET /api/v1/tones?"));
        assert!(String::from_utf8_lossy(&requests[1]).starts_with("GET /api/v1/songs?"));
    }

    #[test]
    fn compatibility_aliases_cover_the_formats_a_device_can_load() {
        assert!(compatible_device("HX Stomp", "HX Stomp"));
        assert!(compatible_device("HX Stomp XL", "HX Stomp"));
        assert!(!compatible_device("HX Stomp", "Helix"));
        assert_eq!(catalog_device_filter("HX Stomp"), "HX Stomp");
        assert_eq!(catalog_device_filter("HX Stomp XL"), "HX Stomp XL,HX Stomp");
    }

    #[test]
    fn publishing_creates_the_song_then_adds_its_tone() {
        let mut song = song_json(12, "Wide Clean");
        song["tone_count"] = 0.into();
        song["tones"] = serde_json::json!([]);
        let server = StubServer::start(vec![(201, song), (201, tone_json(34, 12, "Wide Clean"))]);
        let client = CloudClient::new(&server.base);
        let published = client
            .publish(
                "session-token",
                &PublishRequest {
                    song: PublishSong::New(new_song()),
                    tone: new_tone(),
                },
            )
            .unwrap();
        assert_eq!(published.summary.id, 34);

        let requests = server.finish();
        let song_request = String::from_utf8_lossy(&requests[0]);
        assert!(song_request.starts_with("POST /api/v1/songs "));
        let song_body = song_request.split("\r\n\r\n").nth(1).unwrap();
        let song_body: serde_json::Value = serde_json::from_str(song_body).unwrap();
        assert_eq!(song_body["song"]["kind"], "original");
        assert_eq!(song_body["song"]["title"], "Wide Clean");

        let tone_request = String::from_utf8_lossy(&requests[1]);
        assert!(tone_request.starts_with("POST /api/v1/songs/12/tones "));
        for field in [
            "name=\"tone[name]\"",
            "name=\"tone[series_id]\"",
            "name=\"tone[preset]\"",
            "name=\"tone[blocks][][name]\"",
            "name=\"tone[parsed_metadata][models_used][]\"",
        ] {
            assert!(tone_request.contains(field), "missing {field}");
        }
        for song_only in ["tone[kind]", "tone[artist_name]", "tone[tags]"] {
            assert!(!tone_request.contains(song_only), "sent {song_only}");
        }
    }

    #[test]
    fn adding_to_an_existing_song_skips_song_creation() {
        let server = StubServer::start(vec![(201, tone_json(34, 12, "Wide Clean"))]);
        let client = CloudClient::new(&server.base);
        client
            .publish(
                "session-token",
                &PublishRequest {
                    song: PublishSong::Existing(12),
                    tone: new_tone(),
                },
            )
            .unwrap();
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(String::from_utf8_lossy(&requests[0]).starts_with("POST /api/v1/songs/12/tones "));
    }

    #[test]
    fn a_failed_tone_reports_that_the_song_remains() {
        let mut song = song_json(12, "Wide Clean");
        song["tone_count"] = 0.into();
        song["tones"] = serde_json::json!([]);
        let server = StubServer::start(vec![
            (201, song),
            (422, serde_json::json!({"errors": ["Name can't be blank"]})),
        ]);
        let client = CloudClient::new(&server.base);
        let error = client
            .publish(
                "session-token",
                &PublishRequest {
                    song: PublishSong::New(new_song()),
                    tone: new_tone(),
                },
            )
            .unwrap_err();
        assert!(matches!(
            error,
            PublishError::CreatingTone {
                song_id: 12,
                created_song: Some(_),
                ..
            }
        ));
        assert!(error.to_string().contains("empty Song remains"));
        assert_eq!(server.finish().len(), 2, "no third request is attempted");
    }

    #[test]
    fn external_tones_return_their_source_without_fetching_it() {
        let mut json = tone_json(34, 12, "Wide Clean");
        json["source_kind"] = "external".into();
        json["file_sha256"] = serde_json::Value::Null;
        json["download"] = serde_json::json!({
            "external": "https://line6.com/customtone/tone/34/"
        });
        let tone: ToneDetails = serde_json::from_value(json).unwrap();
        let client = CloudClient::new("http://127.0.0.1:1");
        assert_eq!(
            client.download(&tone).unwrap(),
            ToneDelivery::External("https://line6.com/customtone/tone/34/".into())
        );
    }

    #[test]
    fn stable_tones_decode_their_immutable_version_downloads() {
        let mut json = tone_json(34, 12, "Wide Clean");
        json["series_id"] = "local-series".into();
        json["version_root_id"] = 34.into();
        json["version_number"] = 2.into();
        json["versions_count"] = 2.into();
        json["versions"] = serde_json::json!([
            {
                "number": 1,
                "current": false,
                "file_sha256": "b".repeat(64),
                "created_at": "2026-08-15T10:00:00Z",
                "download": { "artifact": "/tones/34/versions/1/artifact" }
            },
            {
                "number": 2,
                "current": true,
                "file_sha256": "a".repeat(64),
                "created_at": "2026-08-16T10:00:00Z",
                "download": { "artifact": "/tones/34/versions/2/artifact" }
            }
        ]);

        let tone: ToneDetails = serde_json::from_value(json).unwrap();

        assert_eq!(tone.summary.series_id.as_deref(), Some("local-series"));
        assert_eq!(tone.summary.version_number, Some(2));
        assert_eq!(tone.summary.versions_count, 2);
        assert_eq!(tone.versions.len(), 2);
        assert!(tone.versions[1].current);
        assert_eq!(
            tone.versions[0].download.artifact.as_deref(),
            Some("/tones/34/versions/1/artifact")
        );
    }

    #[test]
    fn native_tones_download_the_artifact_selected_from_the_song() {
        let server = StubServer::start_raw(vec![(200, b"native preset".to_vec())]);
        let mut json: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/cloud/tone-details.json"))
                .unwrap();
        json["download"] = serde_json::json!({"artifact": "tones/456/artifact"});
        let tone: ToneDetails = serde_json::from_value(json).unwrap();
        let client = CloudClient::new(&server.base);
        assert_eq!(
            client.download(&tone).unwrap(),
            ToneDelivery::Artifact(b"native preset".to_vec())
        );
        let request = server.finish().pop().unwrap();
        assert!(String::from_utf8_lossy(&request).starts_with("GET /tones/456/artifact "));
    }
}
