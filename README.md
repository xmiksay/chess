# Šachové hádanky

Webová aplikace pro procházení a řešení šachových hádanek z [Lichess puzzle databáze](https://database.lichess.org/#puzzles).

Backend v Rustu (Axum) načte při startu celou CSV databázi do paměti a poskytuje JSON API s filtrováním. Frontend je single-page Vue 3 aplikace s šachovnicí postavenou na [chessground](https://github.com/lichess-org/chessground) a [chess.js](https://github.com/jhlywa/chess.js).

## Features

- 5,9 M hádanek načtených v paměti (~7 s na NVMe + ~4 s indexace)
- Filtrování podle ratingu, tématu, otevření a klíčových slov
- Hard limit **500 výsledků** na dotaz (přizpůsobitelný `--limit`)
- Řazení: vstupní pořadí, náhodně, podle ratingu, popularity, počtu her
- Interaktivní šachovnice — soupeř hraje setup tah, ty hledáš pokračování
- Nápověda (zvýrazní výchozí pole), přehrání řešení, otočení šachovnice
- Validace UCI tahů, automatická promoce na dámu, animace, zvýraznění

## Požadavky

- Rust 1.75+ (stable)
- ~2 GB RAM (pro celou databázi)
- Soubor `lichess_db_puzzle.csv.zst` v pracovním adresáři (lze stáhnout z https://database.lichess.org/lichess_db_puzzle.csv.zst)

## Spuštění

```bash
cargo run --release
# nebo
cargo build --release
./target/release/chess-puzzles
```

Server defaultně naslouchá na `0.0.0.0:3000`. Otevři http://127.0.0.1:3000.

### CLI parametry

| Parametr | Default | Popis |
|---|---|---|
| `--csv` | `lichess_db_puzzle.csv.zst` | Cesta k CSV (podporuje `.zst`) |
| `--bind` | `0.0.0.0:3000` | Adresa pro HTTP server |
| `--static-dir` | `static` | Adresář se statickými soubory |

Logy se ovládají přes `RUST_LOG` (např. `RUST_LOG=info,tower_http=debug`).

## API

### `GET /api/meta`

Metadata o databázi — celkový počet, dostupná témata, rozsah ratingu, hard limit.

### `GET /api/search`

Query parametry:

| Parametr | Typ | Popis |
|---|---|---|
| `rating_min` | u16 | Minimální rating |
| `rating_max` | u16 | Maximální rating |
| `theme` | string | Přesný název tématu (např. `mateIn2`) |
| `opening` | string | Přesný název otevření (např. `Sicilian_Defense`) |
| `q` | string | Volný text — match na id, témata, opening tagy |
| `sort` | string | `rating`, `rating_desc`, `popularity`, `plays`, `random` |
| `limit` | usize | Max výsledků (clamp na 500) |
| `offset` | usize | Stránkování |

Odpověď:

```json
{
  "total_matches": 868374,
  "returned": 50,
  "limit": 50,
  "offset": 0,
  "truncated": true,
  "puzzles": [{ "id": "...", "fen": "...", "moves": "...", "rating": 1500, ... }]
}
```

### `GET /api/puzzle/:id`

Detail jedné hádanky podle PuzzleId.

## Architektura

```
chess/
├── Cargo.toml
├── src/main.rs          # Axum server, CSV loader, search/filter logika
├── static/index.html    # Vue 3 SPA (chessground + chess.js)
└── lichess_db_puzzle.csv.zst
```

### Datový model

Každá hádanka je v paměti uložena jako struktura s `Box<str>` poli:

```rust
struct Puzzle {
    id, fen, moves, themes, game_url, opening_tags: Box<str>,
    rating, rating_deviation: u16,
    popularity: i16,
    nb_plays: u32,
}
```

Při startu se navíc staví `HashMap<id, index>` pro O(1) lookup a `BTreeSet<theme>` pro výpis dostupných témat.

### Vyhledávání

Lineární průchod `Vec<Puzzle>` s aplikací filtrů. Při ~6 M záznamech a jednoduchém filtru typicky < 500 ms (release build, jeden thread). Pro náhodný výběr se používá reservoir-style replacement, takže `&sort=random` neprochází víckrát.

### Logika hádanek (frontend)

Lichess konvence: první tah v `Moves` je „setup" tah soupeře. Aplikace ho přehraje automaticky a uživatel hledá tah druhý. Po správném tahu uživatele se přehraje další tah soupeře, atd. Špatný tah se vrátí (chess.js stav nezměníme, jen se obnoví šachovnice z aktuálního FEN).

User color = opačná strana než `fen.turn()` (protože soupeř hraje první).

## Vývoj

```bash
cargo check               # rychlá kompilační kontrola
cargo build --release     # produkční binárka
RUST_LOG=debug cargo run  # verbose logy
```

Frontend nemá build krok — všechny závislosti jsou z CDN (`unpkg`, `esm.sh`). Pro úpravy editor přepiš `static/index.html` a refresh prohlížeč.
