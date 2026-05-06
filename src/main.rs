use std::{
    collections::{BTreeSet, HashMap},
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::Instant,
};

use anyhow::{Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
    routing::get,
    Router,
};
use clap::Parser;
use serde::{Deserialize, Serialize};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::info;

const HARD_LIMIT: usize = 500;

#[derive(Parser, Debug)]
#[command(about = "Lichess puzzle search server")]
struct Args {
    #[arg(long, default_value = "lichess_db_puzzle.csv.zst")]
    csv: PathBuf,

    #[arg(long, default_value = "0.0.0.0:3000")]
    bind: String,

    #[arg(long, default_value = "static")]
    static_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
struct Puzzle {
    id: Box<str>,
    fen: Box<str>,
    moves: Box<str>,
    rating: u16,
    rating_deviation: u16,
    popularity: i16,
    nb_plays: u32,
    themes: Box<str>,
    game_url: Box<str>,
    opening_tags: Box<str>,
}

#[derive(Deserialize, Debug)]
struct CsvRow {
    #[serde(rename = "PuzzleId")]
    puzzle_id: String,
    #[serde(rename = "FEN")]
    fen: String,
    #[serde(rename = "Moves")]
    moves: String,
    #[serde(rename = "Rating")]
    rating: u16,
    #[serde(rename = "RatingDeviation")]
    rating_deviation: u16,
    #[serde(rename = "Popularity")]
    popularity: i16,
    #[serde(rename = "NbPlays")]
    nb_plays: u32,
    #[serde(rename = "Themes")]
    themes: String,
    #[serde(rename = "GameUrl")]
    game_url: String,
    #[serde(rename = "OpeningTags")]
    opening_tags: String,
}

struct AppState {
    puzzles: Vec<Puzzle>,
    by_id: HashMap<Box<str>, usize>,
    themes: Vec<String>,
    rating_min: u16,
    rating_max: u16,
}

fn load_puzzles(path: &PathBuf) -> Result<Vec<Puzzle>> {
    let start = Instant::now();
    info!("loading puzzles from {}", path.display());

    let file = std::fs::File::open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let buffered = std::io::BufReader::new(file);

    let reader: Box<dyn std::io::Read> = if path.extension().map_or(false, |e| e == "zst") {
        Box::new(zstd::Decoder::new(buffered)?)
    } else {
        Box::new(buffered)
    };

    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(reader);

    let mut puzzles: Vec<Puzzle> = Vec::with_capacity(6_000_000);
    for record in rdr.deserialize::<CsvRow>() {
        let r = record?;
        puzzles.push(Puzzle {
            id: r.puzzle_id.into_boxed_str(),
            fen: r.fen.into_boxed_str(),
            moves: r.moves.into_boxed_str(),
            rating: r.rating,
            rating_deviation: r.rating_deviation,
            popularity: r.popularity,
            nb_plays: r.nb_plays,
            themes: r.themes.into_boxed_str(),
            game_url: r.game_url.into_boxed_str(),
            opening_tags: r.opening_tags.into_boxed_str(),
        });
    }

    info!(
        "loaded {} puzzles in {:?}",
        puzzles.len(),
        start.elapsed()
    );
    Ok(puzzles)
}

fn build_state(puzzles: Vec<Puzzle>) -> AppState {
    let start = Instant::now();
    let mut by_id = HashMap::with_capacity(puzzles.len());
    let mut theme_set = BTreeSet::new();
    let mut rmin = u16::MAX;
    let mut rmax = 0u16;
    for (i, p) in puzzles.iter().enumerate() {
        by_id.insert(p.id.clone(), i);
        for t in p.themes.split(' ') {
            if !t.is_empty() {
                if !theme_set.contains(t) {
                    theme_set.insert(t.to_string());
                }
            }
        }
        rmin = rmin.min(p.rating);
        rmax = rmax.max(p.rating);
    }
    let themes: Vec<String> = theme_set.into_iter().collect();
    info!(
        "indexed {} puzzles, {} unique themes, rating {}..={}, in {:?}",
        puzzles.len(),
        themes.len(),
        rmin,
        rmax,
        start.elapsed()
    );
    AppState {
        puzzles,
        by_id,
        themes,
        rating_min: rmin,
        rating_max: rmax,
    }
}

#[derive(Deserialize, Debug)]
struct SearchParams {
    rating_min: Option<u16>,
    rating_max: Option<u16>,
    theme: Option<String>,
    opening: Option<String>,
    /// space-separated keywords matched against id/themes/opening
    q: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    /// sort: "rating", "popularity", "plays", "random" (default: input order)
    sort: Option<String>,
}

#[derive(Serialize)]
struct SearchResponse {
    total_matches: usize,
    returned: usize,
    limit: usize,
    offset: usize,
    truncated: bool,
    puzzles: Vec<Puzzle>,
}

fn matches(p: &Puzzle, q: &SearchParams, q_lower: Option<&str>) -> bool {
    if let Some(rmin) = q.rating_min {
        if p.rating < rmin {
            return false;
        }
    }
    if let Some(rmax) = q.rating_max {
        if p.rating > rmax {
            return false;
        }
    }
    if let Some(t) = q.theme.as_deref() {
        if !t.is_empty() && !p.themes.split(' ').any(|x| x.eq_ignore_ascii_case(t)) {
            return false;
        }
    }
    if let Some(o) = q.opening.as_deref() {
        if !o.is_empty() && !p.opening_tags.split(' ').any(|x| x.eq_ignore_ascii_case(o)) {
            return false;
        }
    }
    if let Some(qs) = q_lower {
        let id_l = p.id.to_ascii_lowercase();
        let th_l = p.themes.to_ascii_lowercase();
        let op_l = p.opening_tags.to_ascii_lowercase();
        for kw in qs.split_whitespace() {
            if !id_l.contains(kw) && !th_l.contains(kw) && !op_l.contains(kw) {
                return false;
            }
        }
    }
    true
}

async fn search(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SearchParams>,
) -> Json<SearchResponse> {
    let limit = q.limit.unwrap_or(50).min(HARD_LIMIT).max(1);
    let offset = q.offset.unwrap_or(0);
    let q_lower = q.q.as_deref().map(|s| s.to_ascii_lowercase());

    let mut total: usize = 0;
    let mut indices: Vec<usize> = Vec::new();
    let cap = offset.saturating_add(limit);

    for (i, p) in state.puzzles.iter().enumerate() {
        if !matches(p, &q, q_lower.as_deref()) {
            continue;
        }
        total += 1;
        if indices.len() < cap {
            indices.push(i);
        } else if q.sort.as_deref() == Some("random") {
            // reservoir-style replacement so a random subset stays bounded
            let r = (state.puzzles.len().wrapping_mul(i + 1) ^ p.rating as usize) % total;
            if r < cap {
                indices[r] = i;
            }
        } else {
            // input-order: stop once enough collected
            // continue counting only to report total_matches
        }
    }

    match q.sort.as_deref() {
        Some("rating") => indices.sort_by_key(|&i| state.puzzles[i].rating),
        Some("rating_desc") => indices.sort_by_key(|&i| std::cmp::Reverse(state.puzzles[i].rating)),
        Some("popularity") => {
            indices.sort_by_key(|&i| std::cmp::Reverse(state.puzzles[i].popularity))
        }
        Some("plays") => indices.sort_by_key(|&i| std::cmp::Reverse(state.puzzles[i].nb_plays)),
        Some("random") => {
            // fisher-yates style shuffle using rating bytes as cheap entropy
            let len = indices.len();
            for i in (1..len).rev() {
                let j = (state.puzzles[indices[i]].nb_plays as usize ^ i.wrapping_mul(2654435761))
                    % (i + 1);
                indices.swap(i, j);
            }
        }
        _ => {}
    }

    let page: Vec<Puzzle> = indices
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|i| state.puzzles[i].clone())
        .collect();

    Json(SearchResponse {
        total_matches: total,
        returned: page.len(),
        limit,
        offset,
        truncated: total > limit + offset,
        puzzles: page,
    })
}

async fn get_puzzle(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Puzzle>, StatusCode> {
    state
        .by_id
        .get(id.as_str())
        .map(|&i| Json(state.puzzles[i].clone()))
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(Serialize)]
struct MetaResponse {
    total_puzzles: usize,
    themes: Vec<String>,
    rating_min: u16,
    rating_max: u16,
    hard_limit: usize,
}

async fn meta(State(state): State<Arc<AppState>>) -> Json<MetaResponse> {
    Json(MetaResponse {
        total_puzzles: state.puzzles.len(),
        themes: state.themes.clone(),
        rating_min: state.rating_min,
        rating_max: state.rating_max,
        hard_limit: HARD_LIMIT,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let args = Args::parse();

    let puzzles = load_puzzles(&args.csv)?;
    let state = Arc::new(build_state(puzzles));

    let app = Router::new()
        .route("/api/search", get(search))
        .route("/api/puzzle/:id", get(get_puzzle))
        .route("/api/meta", get(meta))
        .with_state(state)
        .fallback_service(ServeDir::new(&args.static_dir))
        .layer(TraceLayer::new_for_http());

    let addr: SocketAddr = args.bind.parse().context("invalid --bind address")?;
    info!("listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
