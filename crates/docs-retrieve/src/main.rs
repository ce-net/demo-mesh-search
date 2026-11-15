//! `docs-retrieve serve`: the docs.retrieve@1 cell behind this machine's door, gated by ce-cap.
//! `docs-retrieve seed <dir>...`: copy collections (`<collection>.jsonl`) into this device's corpus.
//! `docs-retrieve query --grant <link|seed.chain> [--door host:port] <words>`: ask one holder as the grant's key,
//! exactly what the page does in the browser (same key, same Hello, same op).
use ce_wire::blocking::Server;
use ce_wire::Api;
use docs_retrieve_api::*;
use std::sync::Arc;
use std::time::Instant;

fn serve() -> Result<(), String> {
    let me = ce_gate::node_identity().map_err(|e| e.to_string())?;
    let started = Instant::now();
    let (records, bad) = docs_retrieve::load(&docs_retrieve::corpus_dir());
    eprintln!("docs.retrieve: {} records from {} ({} unreadable lines)", records.len(), docs_retrieve::corpus_dir().display(), bad);
    let embed: Option<Box<dyn docs_retrieve::Embedder>> = match std::env::var("DOCS_RETRIEVE_EMBED").as_deref() {
        Ok("off") => None,
        _ => Some(Box::new(docs_retrieve::SearchEmbed)),
    };
    let cell = Arc::new(docs_retrieve::Cell::new(docs_retrieve::Config::from_env(), records, docs_retrieve::device_class(), embed));
    {
        let c = cell.clone();
        std::thread::spawn(move || {
            let t = Instant::now();
            let ok = c.warm();
            eprintln!("docs.retrieve: corpus embedding {} in {} ms", if ok { "ready" } else { "unavailable; ranking is lexical" }, t.elapsed().as_millis());
        });
    }
    let gate = ce_gate::Gate::<DocsRetrieve>::new(me.node_id(), ce_gate::mesh_roots()).into_fn();
    let (c2, c3) = (cell.clone(), cell.clone());
    Server::<DocsRetrieve>::new(gate)
        .on::<HealthOp, _>(move |_: (), _| Ok(Health { ok: true, version: env!("CARGO_PKG_VERSION").into(), uptime_s: started.elapsed().as_secs() }))
        .on::<Search, _>(move |r: SearchReq, c: &ce_wire::Caller| Ok(c2.search(&c.name, &c.rights, r)))
        .on::<Info, _>(move |_: (), _| Ok(c3.info()))
        .serve_door()
        .map_err(|e| e.to_string())
}

fn seed(dirs: &[String]) -> Result<(), String> {
    let to = docs_retrieve::corpus_dir();
    std::fs::create_dir_all(&to).map_err(|e| format!("{}: {e}", to.display()))?;
    for d in dirs {
        let p = std::path::Path::new(d);
        let files: Vec<std::path::PathBuf> = if p.is_dir() { std::fs::read_dir(p).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|f| f.extension().is_some_and(|x| x == "jsonl")).collect() } else { vec![p.to_path_buf()] };
        for f in files {
            let dest = to.join(f.file_name().ok_or("no file name")?);
            std::fs::copy(&f, &dest).map_err(|e| format!("{}: {e}", f.display()))?;
            println!("seeded {} -> {}", f.display(), dest.display());
        }
    }
    let (r, bad) = docs_retrieve::load(&to);
    println!("{} records, {} unreadable lines; restart the cell to load them", r.len(), bad);
    Ok(())
}

fn b64u(s: &str) -> Result<Vec<u8>, String> {
    let mut out = vec![];
    let (mut acc, mut n) = (0u32, 0);
    for ch in s.bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => continue,
            _ => return Err(format!("not base64url: {:?}", ch as char)),
        } as u32;
        acc = (acc << 6) | v;
        n += 6;
        if n >= 8 {
            n -= 8;
            out.push((acc >> n) as u8);
        }
    }
    Ok(out)
}

/// `<page>#g=<seed>.<chain>` or `<seed>.<chain>` -> the browser key and its chain.
fn grant(g: &str) -> Result<(ce_gate::ce_identity::Identity, Vec<ce_gate::ce_cap::SignedCapability>), String> {
    let frag = g.rsplit_once("g=").map(|(_, f)| f).unwrap_or(g);
    let frag = frag.split('&').next().unwrap_or(frag);
    let (s, c) = frag.split_once('.').ok_or("a grant is <seed>.<chain>")?;
    let seed: [u8; 32] = b64u(s)?.try_into().map_err(|_| "the seed must be 32 bytes")?;
    let chain = ce_gate::ce_cap::decode_chain_bytes(&b64u(c)?).map_err(|e| format!("chain: {e}"))?;
    Ok((ce_gate::ce_identity::Identity::from_secret_bytes(&seed), chain))
}

fn query(a: &[String]) -> Result<(), String> {
    let (mut g, mut door, mut json, mut words) = (None, "127.0.0.1:7700".to_string(), false, vec![]);
    let mut it = a.iter();
    while let Some(x) = it.next() {
        match x.as_str() {
            "--grant" => g = it.next().cloned(),
            "--door" => door = it.next().cloned().ok_or("--door needs host:port")?,
            "--json" => json = true,
            w => words.push(w.to_string()),
        }
    }
    let (id, chain) = grant(&g.ok_or("--grant <link|seed.chain> is required")?)?;
    let token = ce_gate::proof_for(&id, &chain, DocsRetrieve::NAME, DocsRetrieve::SCHEMA);
    let t0 = Instant::now();
    let c = ce_wire::blocking::Client::<DocsRetrieve>::connect_opts(door.as_str(), token, false, std::time::Duration::from_secs(5)).map_err(|e| format!("connect: {e}"))?;
    let connect_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let t1 = Instant::now();
    let r = c.call::<Search>(&SearchReq { q: words.join(" "), k: 10 }).map_err(|e| format!("call: {e}"))?.map_err(|e| format!("{e:?}"))?;
    let call_ms = t1.elapsed().as_secs_f64() * 1000.0;
    if json {
        println!("{}", serde_json::json!({"connect_ms": connect_ms, "call_ms": call_ms, "answer": r}));
        return Ok(());
    }
    println!("{} | {} | ranker: {} | scopes: {} | cell {:.2} ms, round trip {:.1} ms (connect {:.1} ms)", r.device, r.cell, r.ranker, r.scopes.join(","), r.took_ms, call_ms, connect_ms);
    for c in &r.counts {
        println!("  {:<14} matched {:>2} before the cap filter, {:>2} after ({} stayed on the device)", c.collection, c.matched, c.visible, c.hidden);
    }
    for h in &r.hits {
        println!("  [{:.3}] {:<8} {} / {}  {}", h.score, h.scope, h.collection, h.id, h.title);
    }
    Ok(())
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let r = match a.get(1).map(String::as_str) {
        Some("serve") => serve(),
        Some("seed") => seed(&a[2..]),
        Some("query") => query(&a[2..]),
        _ => Err("usage: docs-retrieve serve | seed <dir|file.jsonl>... | query --grant <g> [--door host:port] [--json] <words>".into()),
    };
    if let Err(e) = r {
        eprintln!("docs-retrieve: {e}");
        std::process::exit(1);
    }
}
