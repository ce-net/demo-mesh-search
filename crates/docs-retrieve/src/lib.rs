//! The one function of `docs.retrieve@1`: search the records this device holds, as far as the caller's grant reaches.
//!
//! Order matters and is the point of the demo: every record is matched, the match is COUNTED per collection, then
//! every record whose scope the grant does not reach is dropped, and only the rest is ranked, cut and serialised.
//! A hidden record contributes one to a counter and nothing else: no title, no snippet, no score, no id.
//!
//! Serves many callers: every call is counted against the key ce-gate verified (`Caller.name`).
use docs_retrieve_api::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// One record as seeded: a line of `<collection>.jsonl`.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Record {
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default = "public")]
    pub scope: String,
    pub title: String,
    pub text: String,
    #[serde(skip)]
    pub collection: String,
}

fn public() -> String {
    "public".into()
}

/// The corpus directory: `$DOCS_RETRIEVE_CORPUS`, else `~/.local/share/docs-retrieve/corpus`.
pub fn corpus_dir() -> PathBuf {
    std::env::var("DOCS_RETRIEVE_CORPUS").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".local/share/docs-retrieve/corpus")
    })
}

/// Every `*.jsonl` in `dir`; the file stem is the collection. Bad lines are skipped and counted.
pub fn load(dir: &Path) -> (Vec<Record>, u32) {
    let mut out = vec![];
    let mut bad = 0;
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect()).unwrap_or_default();
    files.sort();
    for f in files {
        let coll = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        for l in std::fs::read_to_string(&f).unwrap_or_default().lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<Record>(l) {
                Ok(mut r) => {
                    r.collection = coll.clone();
                    out.push(r)
                }
                Err(_) => bad += 1,
            }
        }
    }
    (out, bad)
}

/// The device class shown to viewers: `$DOCS_RETRIEVE_DEVICE`, else derived from the CPU (never a host name).
pub fn device_class() -> String {
    if let Ok(d) = std::env::var("DOCS_RETRIEVE_DEVICE") {
        if !d.trim().is_empty() {
            return d.trim().to_string();
        }
    }
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let arch = match std::env::consts::ARCH {
        "aarch64" => "ARM",
        "x86_64" => "x86",
        a => a,
    };
    format!("{arch} {}, {cores} cores", std::env::consts::OS)
}

/// The scopes `rights` (as ce-gate gave them: `ability:<a>` entries) reach. `public` is reached by any caller the
/// gate let in for Search; another scope needs `docs.retrieve:scope:<scope>` (or a wildcard that covers it).
pub fn reaches(rights: &[String], scope: &str) -> bool {
    if scope == "public" {
        return true;
    }
    let need = scope_ability(scope);
    rights.iter().filter_map(|r| r.strip_prefix(ce_wire::ABILITY_RIGHT)).any(|a| ce_gate::ce_cap::ability_covers(a, &need))
}

pub fn words(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 2).map(String::from).collect()
}

/// Lexical score: per query word, the best of exact (1.0) or prefix (0.7) among the record's words, title words
/// counting double. 0 = no match.
pub fn lexical(q: &[String], r: &Record) -> f32 {
    if q.is_empty() {
        return 0.0;
    }
    let t = words(&r.title);
    let b = words(&r.text);
    let mut s = 0.0;
    let mut hit = 0;
    for w in q {
        let m = |ws: &[String]| ws.iter().map(|x| if x == w { 1.0 } else if x.starts_with(w.as_str()) { 0.7 } else { 0.0 }).fold(0.0f32, f32::max);
        let v = (2.0 * m(&t)).max(m(&b));
        if v > 0.0 {
            hit += 1;
        }
        s += v;
    }
    if hit == 0 {
        return 0.0;
    }
    // how often the words come back in the body breaks ties between records that match equally well
    let tf = b.iter().filter(|x| q.iter().any(|w| x.starts_with(w.as_str()))).count().min(5) as f32;
    (s / (2.0 * q.len() as f32)) * (0.8 + 0.04 * tf)
}

/// Embeddings from search.embed@1 on this device's door, asked with this node's own proof. None when this device
/// does not run it (then ranking is lexical alone, and the answer says so).
pub trait Embedder: Send + Sync {
    fn embed(&self, texts: &[String], wait: Duration) -> Option<Vec<Vec<f32>>>;
    fn name(&self) -> &'static str;
}

pub struct SearchEmbed;

impl Embedder for SearchEmbed {
    fn embed(&self, texts: &[String], wait: Duration) -> Option<Vec<Vec<f32>>> {
        use ce_wire::Api;
        use search_embed_api::{Embed, EmbedReq, SearchEmbed as A};
        let me = ce_gate::node_identity().ok()?;
        let chain = ce_gate::self_chain(&me, vec!["search.embed:embed".into()], 60);
        let t = ce_gate::proof_for(&me, &chain, A::NAME, A::SCHEMA);
        let door = (std::net::Ipv4Addr::LOCALHOST, std::env::var("CE_DOOR_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(7700u16));
        let c = ce_wire::blocking::Client::<A>::connect_opts(door, t, false, wait).ok()?;
        c.set_deadline(wait);
        c.call::<Embed>(&EmbedReq { texts: texts.to_vec() }).ok()?.ok().map(|r| r.vectors)
    }
    fn name(&self) -> &'static str {
        "lexical + EmbeddingGemma (search.embed@1 on this device)"
    }
}

fn cos(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

pub struct Config {
    pub calls_per_min: u32,
    pub max_callers: usize,
}

impl Config {
    pub fn from_env() -> Config {
        let n = |k: &str, d: u64| std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d);
        Config { calls_per_min: n("DOCS_RETRIEVE_CALLS_PER_MIN", 300) as u32, max_callers: n("DOCS_RETRIEVE_MAX_CALLERS", 10_000).max(1) as usize }
    }
}

pub struct Cell {
    cfg: Config,
    pub records: Vec<Record>,
    pub device: String,
    embedder: Option<Box<dyn Embedder>>,
    /// one vector per record, filled in the background once search.embed answered
    vectors: OnceLock<Vec<Vec<f32>>>,
    /// set once warming gave up: this device runs no embedder (or it refused), so ranking stays lexical
    no_embedder: std::sync::atomic::AtomicBool,
    callers: Mutex<HashMap<String, (Instant, u32)>>,
    /// query vectors already asked for (at most 512): a repeated query costs no model call
    qcache: Mutex<HashMap<String, Vec<f32>>>,
}

pub const CELL: &str = concat!("docs.retrieve@1 v", env!("CARGO_PKG_VERSION"));

impl Cell {
    pub fn new(cfg: Config, records: Vec<Record>, device: String, embedder: Option<Box<dyn Embedder>>) -> Cell {
        Cell { cfg, records, device, embedder, vectors: OnceLock::new(), no_embedder: std::sync::atomic::AtomicBool::new(false), callers: Mutex::new(HashMap::new()), qcache: Mutex::new(HashMap::new()) }
    }

    /// Embed every record once (by title and text). Called from a background thread at start; until it is done,
    /// or when this device has no search.embed, Search ranks lexically.
    pub fn warm(&self) -> bool {
        let Some(e) = &self.embedder else { return false };
        let give_up = || {
            self.no_embedder.store(true, std::sync::atomic::Ordering::Relaxed);
            false
        };
        let texts: Vec<String> = self.records.iter().map(|r| format!("{}. {}", r.title, r.text)).collect();
        let mut all = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(16) {
            match e.embed(chunk, Duration::from_secs(30)) {
                Some(v) if v.len() == chunk.len() => all.extend(v),
                _ => return give_up(),
            }
        }
        self.vectors.set(all).is_ok()
    }

    fn admit(&self, caller: &str) -> Result<(), SearchError> {
        if caller.is_empty() || caller.len() > 256 {
            return Err(SearchError::Invalid("no caller identity".into()));
        }
        let now = Instant::now();
        let mut m = self.callers.lock().unwrap_or_else(|e| e.into_inner());
        if m.len() >= self.cfg.max_callers && !m.contains_key(caller) {
            m.retain(|_, (at, _)| now.duration_since(*at) < Duration::from_secs(60));
            if m.len() >= self.cfg.max_callers {
                return Err(SearchError::Limited("too many callers this minute".into()));
            }
        }
        let s = m.entry(caller.to_string()).or_insert((now, 0));
        if now.duration_since(s.0) >= Duration::from_secs(60) {
            *s = (now, 0);
        }
        if s.1 >= self.cfg.calls_per_min {
            return Err(SearchError::Limited(format!("{} calls a minute per caller", self.cfg.calls_per_min)));
        }
        s.1 += 1;
        Ok(())
    }

    fn collections(&self) -> Vec<String> {
        let mut c: Vec<String> = self.records.iter().map(|r| r.collection.clone()).collect();
        c.sort();
        c.dedup();
        c
    }

    pub fn info(&self) -> InfoResp {
        let counts = self
            .collections()
            .into_iter()
            .map(|c| {
                let all = self.records.iter().filter(|r| r.collection == c).count() as u32;
                let public = self.records.iter().filter(|r| r.collection == c && r.scope == "public").count() as u32;
                CollectionCount { collection: c, matched: all, visible: public, hidden: all - public }
            })
            .collect();
        InfoResp { device: self.device.clone(), cell: CELL.into(), records: self.records.len() as u32, counts }
    }

    /// The one function. `rights`: what the gate gave this caller.
    pub fn search(&self, caller: &str, rights: &[String], r: SearchReq) -> Result<SearchResp, SearchError> {
        let t0 = Instant::now();
        self.admit(caller)?;
        let q = r.q.trim();
        if q.is_empty() || q.len() > 200 {
            return Err(SearchError::Invalid("the query must be 1 to 200 bytes".into()));
        }
        let k = match r.k {
            0 => 10,
            n => n.min(50),
        } as usize;
        let qw = words(q);
        // 1. match everything, 2. count per collection, 3. drop what the grant does not reach -- before any ranking
        let mut counts: Vec<CollectionCount> = self.collections().into_iter().map(|c| CollectionCount { collection: c, matched: 0, visible: 0, hidden: 0 }).collect();
        let mut keep: Vec<(usize, f32)> = vec![];
        for (i, rec) in self.records.iter().enumerate() {
            let lex = lexical(&qw, rec);
            if lex <= 0.0 {
                continue;
            }
            let c = counts.iter_mut().find(|c| c.collection == rec.collection).expect("collection listed");
            c.matched += 1;
            if reaches(rights, &rec.scope) {
                c.visible += 1;
                keep.push((i, lex));
            } else {
                c.hidden += 1;
            }
        }
        // 4. rank only what this caller may see
        let mut ranker = "lexical (this device runs no embedder)";
        if let (Some(e), Some(vs)) = (&self.embedder, self.vectors.get()) {
            let key = qw.join(" ");
            let cached = self.qcache.lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned();
            let qv = cached.or_else(|| {
                let v = e.embed(&[q.to_string()], Duration::from_millis(1500)).and_then(|mut v| v.pop())?;
                let mut c = self.qcache.lock().unwrap_or_else(|e| e.into_inner());
                if c.len() >= 512 {
                    c.clear();
                }
                c.insert(key, v.clone());
                Some(v)
            });
            if let Some(qv) = qv {
                for (i, s) in keep.iter_mut() {
                    *s = 0.6 * *s + 0.4 * cos(&qv, &vs[*i]).max(0.0);
                }
                ranker = e.name();
            } else {
                ranker = "lexical (search.embed@1 did not answer in time)";
            }
        } else if self.embedder.is_some() && !self.no_embedder.load(std::sync::atomic::Ordering::Relaxed) {
            ranker = "lexical (search.embed@1 still embedding the corpus)";
        }
        keep.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
        keep.truncate(k);
        let hits = keep
            .into_iter()
            .map(|(i, score)| {
                let rec = &self.records[i];
                Hit { id: rec.id.clone(), collection: rec.collection.clone(), kind: rec.kind.clone(), scope: rec.scope.clone(), title: rec.title.clone(), snippet: snippet(&rec.text, &qw), score: (score * 1000.0).round() / 1000.0 }
            })
            .collect();
        let mut scopes: Vec<String> = self.records.iter().map(|r| r.scope.clone()).filter(|s| reaches(rights, s)).collect();
        scopes.sort();
        scopes.dedup();
        Ok(SearchResp { device: self.device.clone(), cell: CELL.into(), ranker: ranker.into(), scopes, counts, hits, took_ms: (t0.elapsed().as_secs_f64() * 1000.0 * 100.0).round() / 100.0 })
    }
}

/// About 200 characters of `text` around the first query word.
pub fn snippet(text: &str, q: &[String]) -> String {
    let lower = text.to_lowercase();
    let at = q.iter().filter_map(|w| lower.find(w.as_str())).min().unwrap_or(0);
    let mut start = at.saturating_sub(60);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + 200).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}{}", if start > 0 { "..." } else { "" }, &text[start..end], if end < text.len() { "..." } else { "" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, scope: &str, title: &str) -> Record {
        Record { id: id.into(), kind: "doc".into(), scope: scope.into(), title: title.into(), text: format!("{title} body"), collection: "factory.docs".into() }
    }

    fn cell() -> Cell {
        Cell::new(Config { calls_per_min: 100, max_callers: 10 }, vec![rec("a", "public", "Gearbox overview"), rec("b", "engineer", "Gearbox torque limits"), rec("c", "public", "Paint shop")], "test".into(), None)
    }

    #[test]
    fn guest_sees_public_and_counts_the_rest() {
        let r = cell().search("k", &["ability:docs.retrieve:search".into()], SearchReq { q: "gearbox".into(), k: 10 }).unwrap();
        assert_eq!(r.hits.len(), 1);
        assert_eq!(r.hits[0].id, "a");
        assert_eq!((r.counts[0].matched, r.counts[0].visible, r.counts[0].hidden), (2, 1, 1));
        let wire = serde_json::to_string(&r.hits.iter().map(|h| (&h.id, &h.title, &h.snippet)).collect::<Vec<_>>()).unwrap();
        assert!(!wire.contains("torque"), "a hidden record's content is in the answer: {wire}");
    }

    #[test]
    fn engineer_sees_both() {
        let rights = vec!["ability:docs.retrieve:search".into(), "ability:docs.retrieve:scope:engineer".into()];
        let r = cell().search("k", &rights, SearchReq { q: "gearbox".into(), k: 10 }).unwrap();
        assert_eq!(r.hits.len(), 2);
        assert_eq!(r.counts[0].hidden, 0);
        assert_eq!(r.scopes, vec!["engineer".to_string(), "public".to_string()]);
    }

    #[test]
    fn a_wildcard_grant_reaches_every_scope_and_a_plain_right_does_not() {
        assert!(reaches(&["ability:docs.retrieve:*".into()], "engineer"));
        assert!(!reaches(&["docs.retrieve:scope:engineer".into()], "engineer"), "a right that is not an ability is not a grant");
        assert!(!reaches(&["ability:docs.retrieve:scope:public".into()], "engineer"));
    }

    #[test]
    fn prefix_matches_and_empty_query_refused() {
        assert!(lexical(&words("gear"), &rec("a", "public", "Gearbox")) > 0.0);
        assert!(cell().search("k", &[], SearchReq { q: "  ".into(), k: 0 }).is_err());
    }
}
