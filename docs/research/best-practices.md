# Rust 製 macOS CLI を OSS 公開するためのベストプラクティス（roomy 向け調査）

- 調査日: 2026-10-10。対象は `roomy` の作業ツリー（git はまだコミット 0 件）。
- 目的: 2025〜2026 時点の定番の作法を一次情報で確認し、roomy の現状と突き合わせて
  「今やる / v0.1 までにやる / 後でよい」を決める。
- 表記: **[確認済]** は一次情報（公式ドキュメント・ソース・crates.io API・手元での実行）で確かめたもの。
  **[未検証]** は確認できなかったか、二次情報しか無いもの。
- バージョンは 2026-10-10 に crates.io API / GitHub Releases で確認した最新安定版。

---

## 0. 要点（先に結論）

| # | 推奨 | 理由 |
|---|---|---|
| 1 | **`rust-version` を実態に合わせる（1.88）＋ edition 2024 へ移行** | 宣言は 1.80 だが、lock 済みの clap 4.6.7 / indexmap / hashbrown は 1.85、作業中に追加された ratatui 0.30.2 は 1.88 を要求 [確認済] |
| 2 | **CI を作る**（macOS arm64 で fmt / clippy `-D warnings` / test / MSRV / cargo-deny） | `.github/workflows/` が空。`cargo fmt --check` に差分あり |
| 3 | **`[lints]` テーブルで lint 方針をコード化** | 既定 clippy は警告 0。pedantic では約 70 件（大半がキャスト）。`unsafe` 7 か所に `// SAFETY:` が無い |
| 4 | **終了コード 2 の衝突を解消** | clap の使い方エラーは 2 で終了する [確認済]。`status --exit-code` の「閾値超過＝2」と区別できない |
| 5 | **brew services のラベル `sh.brew.roomy` に対応** | Homebrew 7 では新規・再起動したサービスのラベルが `sh.brew.<formula>`。roomy は `homebrew.mxcl.roomy` しか見ていない [確認済] |
| 6 | **`--json` を「契約」にする** | `top`/`rules` は `schema` 無しの配列、`history`/`doctor` は JSON 非対応、本文が UI 言語で翻訳されて出る |
| 7 | **リリースは自前 workflow（lipo で Universal）＋ 手書き formula（`service do` 付き）** | dist（旧 cargo-dist）は Universal バイナリ非対応・formula に `service` を書けない [確認済] |
| 8 | **署名・公証は v0.3 で良いが、FDA が更新ごとに外れる問題を README に明記** | ad-hoc 署名だと designated requirement が cdhash 依存になり、`brew upgrade` のたびに FDA が外れる |
| 9 | **OSS 必須ファイルを揃える** | `Cargo.toml` の `readme = "README.md"` が存在しない（`cargo package` が失敗する）。LICENSE も無い |

---

## 1. 現状の観察（2026-10-10 時点）

| 項目 | 状態 |
|---|---|
| 構成 | 単一バイナリ crate（`src/main.rs` ＋ 17 モジュール、`src/detect/` 9 ファイル）、約 10k 行 |
| テスト | ユニットテスト 60 件（各モジュール内 `#[cfg(test)]`）＋ `tests/e2e.rs` 9 件。全件成功（e2e 約 15 秒） |
| clippy | 既定設定で警告 0。`-W clippy::pedantic` では約 70 件（キャスト 30 件弱、`too_many_lines` 6 件、`redundant_closure` 6 件など） |
| rustfmt | `cargo fmt --check` で差分あり（`clean.rs` など。並行して編集中のため件数は変動していた） |
| edition / MSRV | `edition = "2021"`, `rust-version = "1.80"`。実際の依存は 1.85 以上（ratatui 追加後は 1.88）を要求 |
| unsafe | `statfs` / `sysctlbyname` / `getuid` ×2 / `flock` / `setiopolicy_np`（自前 `extern "C"`）。`// SAFETY:` コメント無し |
| 非テストの `unwrap`/`expect` | `path.parent().unwrap()`（main.rs, service.rs）、`serde_json::to_string_pretty(..).unwrap()` ×3（output.rs）、Mutex の `lock().unwrap()`、埋め込み資産の `expect`（rules/i18n。テストで担保済みなので許容） |
| `std::env::set_var` | `main()` 冒頭で `GIT_OPTIONAL_LOCKS=0`。edition 2024 では `unsafe fn` になる |
| 依存 | anyhow, chrono, clap 4.6.7, libc, objc2 0.6.5, objc2-foundation 0.3.2, ratatui 0.30（作業中に追加）, rayon, serde, serde_json, toml 0.8.23, unicode-width。ratatui 追加後は macOS 向けに約 107 crate |
| `objc2` | `Cargo.toml` に直接依存として書かれているが `src/` から直接参照なし（不要な可能性。cargo-machete / cargo-shear で確認を） |
| リポジトリ | README / LICENSE / CHANGELOG / CONTRIBUTING / SECURITY / CI すべて未作成。`.gitignore` は `/target` のみ |

---

## 2. プロジェクト構成

### 2.1 有名 CLI の構成 [確認済: 各リポジトリの Cargo.toml / ディレクトリ]

| ツール | 構成 | edition / rust-version | テスト |
|---|---|---|---|
| ripgrep | workspace（`crates/` に grep, ignore, globset ほか。ライブラリを公開するため） | 2024 / 1.96 | `tests/` |
| fd | **バイナリのみ**（`src/main.rs`） | — / 1.90.0 | `tests/tests.rs` ＋ `tests/testenv/` |
| dust | **バイナリのみ** | 2024 / — | `tests/` ＋ fixture ディレクトリ群、assert_cmd |
| zoxide | **バイナリのみ** | 2024 / 1.88.0 | assert_cmd, rstest |
| bat | lib ＋ bin（`src/lib.rs` ＋ `src/bin/`） | 2021 / 1.88 | assert_cmd, predicates |
| just | lib ＋ bin（`[lib] doctest = false`、`[[bin]] test = false`、`tests/lib.rs` に集約） | 2024 / 1.89.0 | 自前ハーネス |
| dua-cli / eza / starship | lib ＋ bin | 2024 | eza は trycmd、dua は snapshot |
| uv | 巨大 workspace（`crates/`） | 2024 / 1.97.0 | — |

### 2.2 roomy への推奨

- **workspace は不要。** 公開ライブラリも複数バイナリも無い。
- **lib/bin 分割は今は不要（後で検討）。**
  - 分割の利点: `tests/` の結合テストから内部 API（worktree 分類、guard、増加検出）を直接呼べる。ベンチや fuzz も書きやすい。
  - 欠点: crates.io に出すとライブラリ部分が公開 API 扱いになる（SemVer の対象が増える）。
  - roomy の e2e は `env!("CARGO_BIN_EXE_roomy")` で実バイナリを `HOME` 差し替えで動かしており、CLI の結合テストとして正しい形。
    内部ロジックのテストは各モジュールの `#[cfg(test)] mod tests` で足りている。
  - 分割するなら just の形（`src/lib.rs` に `pub fn run()`、`main.rs` は数行、`[lib] doctest = false`）が手本。
- **テストの置き場所**
  - ユニット: 今どおりモジュール内。
  - 結合: `tests/e2e.rs`。大きくなったら `tests/e2e/main.rs` ＋ サブモジュールに分ける（`tests/` 直下のファイルはそれぞれ別バイナリとしてビルドされるため、1 バイナリにまとめると速い）。
  - fixture: `tests/fixtures/` に外部コマンドの出力サンプル（`docker system df`、`mdls`、`git status --porcelain`、`.obsolete`）を置き、パーサのテストで `include_str!` する。
- **CLI テスト用 crate**（すべて dev-dependency）

| crate | 最新 | MSRV | 用途 | roomy での使いどころ |
|---|---|---|---|---|
| `assert_cmd` | 2.2.2 | 1.85 | バイナリ実行とアサーション | 既存の `Sandbox` ヘルパーで足りている。乗り換えは任意 |
| `predicates` | 3.1.4 | — | `contains` などの述語 | assert_cmd と組み合わせ |
| `assert_fs` | 1.1.4 | — | 一時ディレクトリと fixture | `tempfile` で足りている |
| `insta` | 1.49.0 | 1.66 | スナップショット（JSON/YAML、redaction） | **推奨**: `report --json` の形を固定し、意図しない schema 変更を検出 |
| `trycmd` / `snapbox` | 1.2.1 / 1.2.2 | — | `.toml`/`.md` に書いたコマンド例をそのまま検証 | `--help` や README の例の検証（後で） |
| `proptest` | 1.11.0 | — | プロパティテスト | guard（パス正規化）向き（後で） |

insta の例（時刻やパスなど変わる値は redaction で伏せる）:

```rust
// tests/e2e.rs
insta::assert_json_snapshot!(report(&sb), {
    ".time" => "[time]",
    ".candidates[].path" => "[path]",
});
```

---

## 3. コードスタイルとイディオム

### 3.1 エラー処理

| 層 | 使うもの | 理由 |
|---|---|---|
| バイナリの最上位（`main` / `run` / コマンド実装） | `anyhow` 1.x（`Context` で文脈を足す） | 呼び出し側でエラーの種類を分岐しない。`{e:#}` で原因の連鎖を表示（roomy は実装済み） |
| 呼び出し側が種類で分岐するもの | `thiserror` 2.0.21 の enum | 例: guard の拒否理由、clean の再検証結果、設定の読み込み失敗 |
| 「無いのが普通」 | `Option` | 外部ツールが無い、レポートがまだ無い、など（roomy の方針どおり） |

- roomy の `config::parse -> Result<_, String>` は小規模なら許容。理由ごとに表示を変えたくなったら thiserror に移す。
- `state::load` が読み込み失敗（JSON 破損）を `None` にまとめている点に注意。「レポートが無い」と「壊れている」の区別がつかない。
  `doctor` で破損を検出するか、`Result<Option<T>>` にする。

### 3.2 `unwrap` / `expect`

- 非テストコードでは原則使わない。使うなら「なぜ失敗しないか」を `expect("…")` の文言に書く。
- lint で強制する（テストでは許可）:

```toml
# clippy.toml
allow-unwrap-in-tests = true
allow-expect-in-tests = true
```

- roomy で直す候補:
  - `path.parent().unwrap()` → `.context("config path has no parent")?`
  - `serde_json::to_string_pretty(..).unwrap()` → 表示関数を `anyhow::Result<()>` に（stdout が閉じたときの `println!` の panic 対策にもなる）
  - `Mutex::lock().unwrap()` は poison 時に panic するのが通例なので許容（または `parking_lot`、不要なら変えない）
  - 埋め込み資産（`rules/builtin.toml`, `locales/*.toml`）の `expect` はテストで担保されているので許容

### 3.3 型と属性

- **`#[must_use]`**: pedantic の `must_use_candidate` は誤検知が多いので全体には付けず、捨てるとバグになる関数に限る。
  - 最優先は `sys::try_lock`。`let _ = try_lock(..)` と書くとロックが即座に外れる。

```rust
#[must_use = "the lock is released as soon as the returned File is dropped"]
pub fn try_lock(path: &Path) -> Option<File> { … }
```

- **newtype / enum**（後でよい）:
  - `memory_pressure: i64`（1/2/4 の魔法の数）→ `enum Pressure { Normal, Warn, Critical, Unknown }`
  - バイト数 `u64` → `struct Bytes(u64)`（表示 `fmt_bytes` をメソッドに。GB と GiB の取り違え防止）
  - 候補 ID / ルール ID → `struct CandidateId(String)`
- **可視性**: バイナリ crate では `pub` も実質 crate 内限定。`unreachable_pub` lint は lib 分割したときに有効にすればよい。
- **ドキュメント**: 全モジュールに `//!` があり良い状態。公開関数の `///` は「何を返すか・失敗条件」を中心に。
  `cargo doc --document-private-items` で読めるようにしておくと貢献者が助かる。

### 3.4 lint 設定（`[lints]` テーブル、Cargo 1.74+）[確認済: Cargo Book / just・uv の Cargo.toml]

just は `pedantic = deny`、uv は `pedantic = warn` に除外リストを付けている。roomy では次の形を推奨（CI で `-D warnings`）:

```toml
# Cargo.toml
[lints.rust]
unsafe_op_in_unsafe_fn = "deny"   # edition 2024 では既定で warn
unused_qualifications = "warn"

[lints.clippy]
all = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
# pedantic のうち、このコードでは雑音になるもの
cast_possible_truncation = "allow"
cast_precision_loss = "allow"      # バイト数→f64 の表示用変換
cast_sign_loss = "allow"
cast_possible_wrap = "allow"
module_name_repetitions = "allow"
must_use_candidate = "allow"
missing_errors_doc = "allow"
missing_panics_doc = "allow"
too_many_lines = "allow"
struct_excessive_bools = "allow"   # clap の Opts
similar_names = "allow"
# restriction から選んで有効化
undocumented_unsafe_blocks = "warn"
unwrap_used = "warn"
dbg_macro = "warn"
todo = "warn"
```

- clippy は `Cargo.toml` の `rust-version` を MSRV として読むので、`clippy.toml` に `msrv` は不要。
- いきなり pedantic を有効にすると約 70 件出る。`cargo clippy --fix` で直る分（約 30 件）を先に片付けてから有効にする。

### 3.5 rustfmt

- 設定は既定のままでよい（`rustfmt.toml` は不要）。edition 2024 にすると `style_edition = "2024"` が自動で使われる。
- CI に `cargo fmt --all --check`。並行編集が多いなら pre-commit（`cargo fmt`）も。

### 3.6 MSRV と edition

- **事実** [確認済: 各 crate の `rust-version`]:
  - clap 4.6.7 / clap_lex 1.1.1 / indexmap 2.14.2 / hashbrown 0.17.1 / getrandom 0.4.3 → **1.85**
  - ratatui 0.30.2 / time 0.3.55 → **1.88**
  - 現在の stable は **1.99.0**（2026-09-28、`channel-rust-stable.toml`）。x86_64-apple-darwin は 1.90 から Tier 2（host tools 付き）に格下げ
- **推奨**: `edition = "2024"`, `rust-version = "1.88"`。方針は「stable の 6〜8 リリース前まで（約 1 年）。上げるときは minor バージョンを上げて CHANGELOG に書く」。
  - 主要 CLI の水準: ripgrep 1.96、uv 1.97、starship 1.95、fd・eza 1.90、just 1.89、zoxide・bat 1.88。
  - 配布はビルド済みバイナリが中心で、MSRV が効くのは `cargo install` するユーザーだけ。高めでも実害は小さい。
- edition 2024 では resolver v3（MSRV を考慮する resolver）が既定になる。`cargo update` が `rust-version` に合う依存を優先して選ぶ。
- CI に MSRV ジョブ（`dtolnay/rust-toolchain@1.88` で `cargo check --locked --all-targets`）。

### 3.7 edition 2024 移行で roomy に必要な変更

`cargo fix --edition` の後、手で直すもの:

| 箇所 | 2024 での扱い | 対応 |
|---|---|---|
| `std::env::set_var("GIT_OPTIONAL_LOCKS", "0")`（main.rs） | `unsafe fn` になる（マルチスレッドで未定義動作になりうる） | **プロセス環境を書き換えず、git を起動する `Command` ごとに `.env("GIT_OPTIONAL_LOCKS", "0")`**。`sys::run` に env 引数を足すか、git 専用ヘルパーを作る |
| `extern "C" { fn setiopolicy_np(..) }`（sys.rs） | `unsafe extern "C"` が必須。引数が整数だけなので `safe fn` 宣言にできる | 下の例。libc 0.2.190 には `setiopolicy_np` が無いので自前宣言のままでよい [確認済] |
| `unsafe fn` 内の unsafe 操作 | `unsafe_op_in_unsafe_fn` が既定で warn | lint を deny にして明示する |
| `if let` の一時値の寿命、RPIT の lifetime capture | 意味が変わる | `cargo fix --edition` が多くを自動で直す。テストで確認 |

```rust
// SAFETY: setiopolicy_np takes plain integers and only changes this process's I/O policy.
unsafe extern "C" {
    safe fn setiopolicy_np(iotype: libc::c_int, scope: libc::c_int, policy: libc::c_int) -> libc::c_int;
}
```

### 3.8 `unsafe` の衛生

- すべての `unsafe { }` の直前に `// SAFETY:` で前提を書く（`clippy::undocumented_unsafe_blocks` で強制）。
- `libc::getuid()` は失敗しないので、`sys::uid()` という安全な関数に 1 か所だけ包む（今は service.rs と sys.rs に重複）。
- `statfs` / `sysctlbyname` は「バッファ長を先に問い合わせ、返り値を確認してから読む」ことをコメントに残す。
- objc2-foundation の `trashItemAtURL_resultingItemURL_error` は safe API として呼べているので、そのままでよい。

---

## 4. CLI の UX 規約

### 4.1 終了コード

- Rust CLI Book は「0 = 成功、非 0 = 失敗。可能なら BSD sysexits の意味に合わせる（`exitcode` crate）」を推奨。panic は 101。
- **clap は使い方エラーで 2 を返す**（`clap_builder` の `USAGE_CODE = 2`）[確認済: clap_builder 4.6.7 のソース]。
- roomy の設計では「2 = 閾値超過（`status --exit-code`）」なので、**スクリプトから見ると引数ミスと閾値超過を区別できない**。

推奨する表（README と `--help` に書く）:

| コード | 意味 |
|---|---|
| 0 | 正常 |
| 1 | 実行時エラー（設定破損、レポート無し、削除失敗、doctor で問題あり） |
| 2 | 使い方の誤り（clap に任せる） |
| 3 | `status --exit-code` で閾値超過（2 から変更） |
| 101 | panic（バグ） |

### 4.2 stdout と stderr

- **stdout = 結果（パイプで次に渡すもの）、stderr = 進捗・警告・エラー。** roomy は `scan.walking` を stderr、設定破損の警告を stderr に出しており良い。
- 直す候補: `clean` の個別失敗（`✗ … : error`）とガード拒否は stderr に出す（今は stdout。e2e の `stdout(&o).contains("protected")` も合わせて変える）。
- `--json` のときは stdout に JSON 以外を一切出さない（今の実装はそうなっている）。

### 4.3 `--json` の安定性

DESIGN.md §3 は「全コマンド `--json`、`{"schema": 1, ...}`」としているが、実装は次のとおり:

| コマンド | 現状 | 推奨 |
|---|---|---|
| `status --json` | `schema` あり | そのまま |
| `scan/report --json` | `schema` あり。ただし **title / evidence / notes が UI 言語で翻訳済みの文字列** | 機械向けは言語非依存にする。JSON は常に英語で出すか、`"title": {"key": "...", "args": {...}, "text": "..."}` のようにキーも併記 |
| `top --json` / `rules --json` | **schema 無しの素の配列** | `{"schema":1,"entries":[...]}` に包む（配列のままだと後から項目を足せない） |
| `history` / `doctor` / `config show` | JSON 非対応 | `history --json`（JSON Lines でも可）、`doctor --json`（チェック名・結果・ヒント） |

互換性の規則（README の「JSON 出力」節に書く）:
- フィールドの**追加**は schema を上げない。利用側は未知のフィールドを無視する前提。
- 削除・改名・型や意味の変更は `schema` を上げ、CHANGELOG に Breaking として書く。
- バイト数は整数（10 進バイト）、時刻は Unix 秒（または RFC 3339）で統一し、単位をフィールド名か文書で明示する。

### 4.4 色と端末判定

- 仕様 [確認済]:
  - NO_COLOR: 「存在して**空でない**とき」色を付けない。ユーザー設定やコマンドラインのフラグが優先する。対象は色だけで、太字などは対象外。
  - CLICOLOR_FORCE（空でない）: 端末でなくても色を付ける。CLICOLOR=0: 色を付けない。
- roomy の `Sty::new()` の差分:
  - `NO_COLOR=""`（空）でも色を消してしまう（仕様では無視すべき）
  - `CLICOLOR_FORCE` / `--color always|never|auto` が無い
  - stderr に出す色も stdout の tty 判定を使っている
- **推奨**: clap が既定で依存している `anstream` 1.0.0 / `anstyle` 1.0.14 を直接使う（依存の追加は実質なし）。
  `anstream::stdout()` が NO_COLOR / CLICOLOR / CLICOLOR_FORCE / tty を処理し、ANSI を取り除いて書き出す。
  グローバルフラグ `--color <auto|always|never>` を足し、`anstream::ColorChoice::write_global()` に渡す。
- TUI（ratatui）を入れるなら、tty でないとき・`--json` のときは TUI を起動しない分岐を必ず入れる。

### 4.5 補完とマニュアル

| crate | 最新 | MSRV | 用途 |
|---|---|---|---|
| `clap_complete` | 4.6.11 | 1.85 | bash/zsh/fish/elvish/PowerShell の補完 |
| `clap_mangen` | 0.3.3 | — | `roff` 形式の man ページ |

- 手本: ripgrep は `rg --generate man|complete-zsh…`、just は `just --completions <shell>` のように**バイナリ自身が生成**する。
  build.rs で生成するより単純で、Homebrew の formula からも呼べる。
- roomy では隠しサブコマンド `roomy completions <shell>` と `roomy man` を追加。formula では次のように書く:

```ruby
generate_completions_from_executable(bin/"roomy", "completions")
(man1/"roomy.1").write Utils.safe_popen_read(bin/"roomy", "man")
```

（`generate_completions_from_executable` は手元の Homebrew 7.0.9 の `formula.rb` で確認済み）

### 4.6 `--version`

- clap の `version` だけだと `roomy 0.1.0`。不具合報告用に `long_version` でコミット・ターゲット・ビルド日を出す（`roomy -V` は短く、`roomy --version` は詳しく）。
  - 取得方法は build.rs で `git rev-parse --short HEAD` を `cargo:rustc-env` に出すか、CI で環境変数を渡して `option_env!` で読む。
  - リリースの tarball（git 履歴無し）からビルドしても壊れないよう、取れなければ `unknown` にする。
- `doctor` の先頭にも同じ情報を出すと、Issue に貼ってもらいやすい。

### 4.7 設定ファイルの場所（macOS）

| 方式 | 例 | 備考 |
|---|---|---|
| `~/.config/<app>`（XDG を尊重） | gh, starship, bat など多くの開発者向け CLI | dotfiles で持ち運びやすい。**roomy の現行方式で良い** |
| `~/Library/Application Support/<app>` | `dirs::config_dir()` の macOS での戻り値 | 開発者には見つけにくい。設定には使わない |

- 状態ファイルを `~/Library/Application Support/roomy`、設定を `~/.config/roomy` に置く現行の分け方は妥当。
- crate を使うなら `etcetera` 0.11.0 の Xdg 戦略（macOS でも `~/.config`）。`dirs` 7.0.0 は macOS で Application Support を返すので設定用には不向き。今の手書き実装で十分。
- `ROOMY_CONFIG_DIR` / `ROOMY_STATE_DIR` は「テスト用」と書かれているが、README の環境変数一覧に載せるか、`#[doc(hidden)]` 扱いと明記する。

### 4.8 ログ

- 現状はログ基盤なし。agent は 1 行の要約を stdout に出し、launchd がそれを `logs/agent.log` に書き、1 MB でローテートしている。
- **v0.1 はこのままでよい。** 足すなら:
  - グローバル `-v/--verbose`（回数）と `-q/--quiet`。診断は stderr へ。
  - agent のログ行に時刻（RFC 3339）を付ける。
- 後で基盤を入れるなら:
  - 小さく済ませる: `log` 0.4.34 ＋ `env_logger` 0.11.11
  - 構造化ログ: `tracing` 0.1.44 ＋ `tracing-subscriber` 0.3.23（`EnvFilter` に `ROOMY_LOG` を読ませる）
  - 環境変数は `RUST_LOG` より `ROOMY_LOG` のように固有名にすると、他の Rust 製ツールと干渉しない

### 4.9 多言語化（i18n）

| 方式 | 例 | 長所 | 短所 |
|---|---|---|---|
| 手書きのカタログ（roomy の現行） | `locales/*.toml` ＋ `t()/tf()` | 依存ゼロ。キー網羅・プレースホルダ一致・未定義キーをテストで検証済み。保存時はキー、表示時に翻訳するので agent と端末の言語が違っても正しい | 複数形・性などの文法は自前 |
| `rust-i18n` 4.2.4 | `t!("key", name = v)` マクロ | コンパイル時に埋め込み、YAML/JSON/TOML | マクロ依存。roomy の「保存は後で翻訳」には合わない |
| Fluent（`fluent` 0.17 / `fluent-templates` 0.15.1） | Firefox など | 複数形・選択式が言語ごとに書ける | 依存と学習コストが大きい |

- **推奨: 現行の手書き方式を続ける。** 英語と日本語だけで、日本語は複数形が要らない。
  英語の "1 items" のような複数形だけ、`{n}` を受ける専用キー（`…_one` / `…_other`）で対処する。
- 言語検出で launchd 下では `defaults read -g AppleLanguages` を毎回起動している。
  後で objc2-foundation の `NSLocale::preferredLanguages` に置き換えれば子プロセスが不要になる（feature `NSLocale` の追加が必要。[未検証]: API 名は objc2-foundation 0.3 のドキュメントで要確認）。
- clap の `--help` は英語のみ。CLI では一般的なので許容し、README（日本語版）で補う。

---

## 5. 品質とサプライチェーン

### 5.1 CI の環境（macOS）[確認済: GitHub Docs「GitHub-hosted runners」]

| ラベル | アーキテクチャ | 備考 |
|---|---|---|
| `macos-latest` | arm64（M1, 3 コア / 7 GB） | ドキュメント上は macOS 26 イメージへのリンク |
| `macos-14` / `macos-15` / `macos-26` | arm64 | |
| `macos-15-intel` / `macos-26-intel` | Intel（4 コア / 14 GB） | Intel のランナーは 2027 年秋に終了予定（Homebrew 7.0.0 の告知。月は資料により 8 月〜秋で揺れがある [未検証]） |

- Intel を取り巻く状況:
  - Rust: x86_64-apple-darwin は 1.90 で Tier 2 に格下げ [確認済]
  - Homebrew: 2026-09 から Intel は Tier 3（bottle の新規ビルドなし）、2027-09 に Intel のサポート終了 [確認済]
  - Apple: macOS 27 で Intel 非対応（Homebrew 7.0.0 の記述）
- **推奨**:
  - 必須ジョブは arm64（`macos-latest`）だけ
  - Intel 版は arm64 ランナー上で**クロスコンパイル**してリリースに含める
  - `macos-15-intel` でのテストは週 1 回の任意ジョブにし、2027 年に外す

### 5.2 推奨ワークフロー（`.github/workflows/ci.yml`）

action の最新: `actions/checkout` v7.0.1、`Swatinem/rust-cache` v2.9.2、`EmbarkStudios/cargo-deny-action` v2.1.1、`taiki-e/install-action` v2、`dtolnay/rust-toolchain`（ブランチ指定）。

```yaml
name: CI
on:
  push: { branches: [main] }
  pull_request:
  schedule: [{ cron: "0 3 * * 1" }]   # 週 1 回（advisory と Intel）
permissions: { contents: read }
env: { CARGO_TERM_COLOR: always }

jobs:
  test:
    runs-on: macos-latest            # arm64
    steps:
      - uses: actions/checkout@v7
      - uses: dtolnay/rust-toolchain@stable
        with: { components: "rustfmt, clippy" }
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy --locked --all-targets -- -D warnings
      - run: cargo test --locked
  msrv:
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v7
      - uses: dtolnay/rust-toolchain@1.88
      - uses: Swatinem/rust-cache@v2
      - run: cargo check --locked --all-targets
  intel:
    if: github.event_name == 'schedule'
    runs-on: macos-15-intel
    steps:
      - uses: actions/checkout@v7
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --locked
  deny:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: EmbarkStudios/cargo-deny-action@v2
```

- 外部の action はコミット SHA で固定し、Dependabot に更新させる（OpenSSF Scorecard の推奨）。上の例はタグ表記で簡略化している。
- e2e は `HOME` を一時ディレクトリにするので、CI でも実ユーザーの領域には触れない。TCC ダイアログも出ない（`skipped_protected` の検証あり）。

### 5.3 依存の監査

| ツール | 最新 | 役割 | 推奨 |
|---|---|---|---|
| `cargo-deny` | 0.20.2 | advisories（RustSec）・ライセンス・重複・取得元 | **採用**（cargo-audit の機能を含む） |
| `cargo-audit` | 0.22.2 | RustSec advisories のみ | cargo-deny を使うなら不要 |
| `cargo-machete` | 0.9.2 | 未使用依存の検出（速いが推測ベース） | 時々手で実行。`objc2` の直接依存が不要か確認 |
| `cargo-shear` | 1.14.0 | 未使用依存の検出（コードを解析） | machete の代わりに CI でも可 |
| `cargo-udeps` | — | nightly が必要 | 不要 |

macOS 向けの依存ツリー（ratatui 込み、約 107 crate）に出てくるライセンスから作った最小構成 [確認済: cargo metadata]:

```toml
# deny.toml
[graph]
targets = ["aarch64-apple-darwin", "x86_64-apple-darwin"]

[licenses]
allow = ["MIT", "Apache-2.0", "Unicode-3.0", "Zlib"]
confidence-threshold = 0.9

[bans]
multiple-versions = "warn"
wildcards = "deny"

[sources]
unknown-registry = "deny"
unknown-git = "deny"
```

`cargo deny init` で雛形を作ってから上の値を入れるのが確実（キー名の細部は cargo-deny のバージョンで変わることがある）。

- ratatui 0.30 は既定 feature（`all-widgets`, `macros`, `layout-cache`, `underline-color`）を有効にする。
  使う範囲が決まったら `default-features = false, features = ["crossterm"]` から必要なものだけ足すと、依存とビルド時間が減る。
- **Dependabot**（`.github/dependabot.yml`）: `cargo` と `github-actions` を週 1 回。`groups` で minor/patch をまとめる。
  Renovate は lockfile のメンテナンスや自動マージが柔軟だが、個人の小規模リポジトリなら Dependabot で十分。

### 5.4 バージョン付けと CHANGELOG

| 方式 | 最新 | 内容 | roomy への適合 |
|---|---|---|---|
| **release-plz** | 0.3.170（action v0.5.140） | Conventional Commits から次のバージョンと CHANGELOG（git-cliff で生成）を作り、リリース PR を開く。マージでタグを打つ | **推奨**。バイナリのみなら `publish = false`, `git_only = true`, `semver_check = false`。GitHub Release は下のリリース workflow に任せるため `git_release_enable = false` |
| release-please | v5.0.0 | Google 製。言語非依存 | 可。Rust 固有の機能は少ない |
| 手動 ＋ git-cliff | 2.14.2 | `git cliff --bump` で CHANGELOG を作る | 最小構成ならこれでも十分 |

```toml
# release-plz.toml
[workspace]
publish = false
git_only = true
semver_check = false
git_release_enable = false
release_always = false
```

- コミットは Conventional Commits 1.0（`feat:` / `fix:` / `feat!:`）。ルール追加は `feat(rules): …` のように scope を付けると CHANGELOG が読みやすい。
- CHANGELOG は Keep a Changelog 1.1.0 の形（Added / Changed / Fixed / Removed / Security）。
- **CLI の SemVer**: 公開インターフェースを明文化する。
  - 対象: サブコマンドとフラグ、終了コード、`--json` の schema、設定キー、状態ファイルと LaunchAgent の場所・ラベル
  - 0.x のうちは minor で破壊的変更してよいが、CHANGELOG に Breaking として書く
  - 1.0 以降は、これらの破壊的変更で major を上げる

### 5.5 リリース（GitHub Releases ＋ Homebrew tap）

**dist（旧 cargo-dist）の評価**:
- 最新は GitHub Releases の 0.33.0（2026-09-10）。crates.io の `cargo-dist` は 0.32.0 で止まっている
- 機能: tarball、sha256、GitHub Release、shell installer、Homebrew formula の tap への publish（`HOMEBREW_TAP_TOKEN`）、`github-attestations = true`

| 要件 | dist の対応 [確認済: リポジトリのソースと Issue] |
|---|---|
| Universal バイナリ（lipo） | **非対応**（Issue #77「macOS: Create universal binaries」が 2023 年から open） |
| formula に `service do` / 補完 / `test do` | **書けない**（`templates/installer/homebrew.rb.j2` に該当箇所なし。本の Homebrew の章もカスタム内容に触れていない） |
| macOS のコード署名 | ある: `macos-sign = true` ＋ 環境変数 `CODESIGN_IDENTITY` / `CODESIGN_CERTIFICATE`（base64 の p12）/ `CODESIGN_CERTIFICATE_PASSWORD` / `CODESIGN_OPTIONS`（`runtime` で hardened runtime）。ただし本には macOS 署名のページが無い |
| 公証（notarytool） | **無い**（リポジトリに `notarytool` の記述なし） |

**結論**: roomy は macOS 専用で「Universal ＋ `service` 付き formula」が要件なので、**自前の release workflow（100 行弱）＋ 手書き formula** の方が単純で見通しが良い。
dist はマルチ OS のインストーラを配る場合に真価があり、roomy では利点が少ない。

```yaml
# .github/workflows/release.yml（要点のみ）
on: { push: { tags: ["v*"] } }
permissions: { contents: write, id-token: write, attestations: write }
jobs:
  release:
    runs-on: macos-latest
    env: { MACOSX_DEPLOYMENT_TARGET: "13.0" }   # DESIGN の対応 OS に合わせる
    steps:
      - uses: actions/checkout@v7
      - uses: dtolnay/rust-toolchain@stable
        with: { targets: "aarch64-apple-darwin,x86_64-apple-darwin" }
      - run: |
          cargo build --release --locked --target aarch64-apple-darwin
          cargo build --release --locked --target x86_64-apple-darwin
          mkdir -p dist
          lipo -create -output dist/roomy \
            target/aarch64-apple-darwin/release/roomy \
            target/x86_64-apple-darwin/release/roomy
          lipo -archs dist/roomy                       # "x86_64 arm64" を確認
          # 署名する場合はここで codesign（lipo の後に行う）
          cp README.md LICENSE-MIT LICENSE-APACHE CHANGELOG.md dist/
          tar -C dist -czf roomy-${GITHUB_REF_NAME}-universal-apple-darwin.tar.gz .
          shasum -a 256 roomy-*.tar.gz > roomy-${GITHUB_REF_NAME}.sha256
      - uses: actions/attest-build-provenance@v4
        with: { subject-path: "roomy-*.tar.gz" }
      - run: gh release create "$GITHUB_REF_NAME" roomy-*.tar.gz roomy-*.sha256 --notes-file <(…CHANGELOG の該当節…)
        env: { GH_TOKEN: "${{ github.token }}" }
      # 続けて tap リポジトリの Formula/roomy.rb の url / sha256 / version を書き換えて push（tap 用の PAT が必要）
```

- `MACOSX_DEPLOYMENT_TARGET` を設定しないと、Rust の既定（aarch64 は 11.0、x86_64 は 10.12）でビルドされる。
  動作上は問題ないが、DESIGN の「macOS 13 以降」に揃えると、新しい API（`statfs` 系など）を使っても差異が出ない。[未検証: 既定値は Rust の platform support ページで要確認]
- 署名する場合は **lipo の後に** universal バイナリへ `codesign` する（`codesign` はスライスごとに署名する）。
  arm64 の Rust バイナリはリンカが ad-hoc 署名済みで、lipo 後も各スライスの署名は残る見込み。
  [未検証] 念のため `codesign -v` で確認するか、`codesign -s - -f` で付け直す。

### 5.6 コード署名・公証と TCC（フルディスクアクセス）

- **TCC は許可を「コードの designated requirement（DR）」に結び付ける。**
  - ad-hoc 署名のコードには安定した DR が無く、cdhash（バイナリ内容のハッシュ）で識別される
  - そのため**ビルドし直す・`brew upgrade` するたびに FDA の許可が実質外れる**（設定画面には残って見えることがある）[確認済: Apple TN3127 / Apple Developer Forums の DTS 回答]
- Developer ID で署名すると DR は「識別子 ＋ Team ID」になり、**バージョンを跨いで FDA が維持される**。
- 必要なもの（[未検証] を含む。実施時に Apple のドキュメントで再確認）:
  - Apple Developer Program（年額。日本では 12,980 円 [未検証]）
  - 証明書: Developer ID Application
  - 署名: `codesign --sign "Developer ID Application: …" --options runtime --timestamp --identifier io.github.masa-tichita.roomy roomy`
  - 公証: zip にして `xcrun notarytool submit --wait`。**単体の Mach-O にはチケットを staple できない**ので、Gatekeeper はオンラインでチケットを確認する [未検証: Apple「Customizing the notarization workflow」で要確認]
  - Linux や CI で証明書を扱うなら `rcodesign`（apple-codesign 0.29.0）でも署名と公証ができる
- Gatekeeper との関係:
  - Homebrew の formula は curl でダウンロードするため quarantine 属性が付かず、未署名でも Gatekeeper に止められない（広く知られた挙動だが一次情報は未確認 [未検証]）
  - ブラウザで Releases から落とした tarball は quarantine が付くので、未署名だと実行時にブロックされる。README に `xattr -d com.apple.quarantine` の手順を書く
- Homebrew 5.0 で「署名の無い cask は非推奨」、2026-09 に Gatekeeper を通らない cask を公式 cask リポジトリで無効化する方針 [確認済]。
  **未署名のうちは cask ではなく formula で配る。**
- macOS 13 以降、LaunchAgent を置くと「バックグラウンド項目が追加されました」の通知が出て、ログイン項目の設定に表示される。
  表示名は署名の開発者名から取られるらしく、未署名だと実行ファイル名などになる [未検証]。README に「この通知は roomy の agent」と書いておく。
- **方針**: DESIGN どおり v0.3 で署名・公証を行う。それまでは README と `doctor` で「アップグレード後は FDA を付け直す（設定画面で一度外して再追加）」と案内する。
  FDA が無くても主要機能が動く設計（agent は保護領域を走査しない）は、この制約に対して正しい。

### 5.7 その他の供給網対策（後でよい）

- `actions/attest-build-provenance` v4 で成果物に来歴を付ける。利用者は `gh attestation verify roomy-….tar.gz -R masa-tichita/roomy` で検証できる。
  Homebrew 7 は対応する third-party tap の bottle の attestation を検証する [確認済: 7.0.0 告知]。
- `cargo-auditable` 0.7.7: 依存一覧をバイナリに埋め込み、後から `cargo audit bin` で監査できる。
- GitHub の「Private vulnerability reporting」を有効化（SECURITY.md から案内）。

---

## 6. OSS リポジトリの体裁

### 6.1 置くファイル

| ファイル | 内容 | 参考 |
|---|---|---|
| `README.md` / `README.ja.md` | 下記の構成 | just は `README.中文.md` を併置 |
| `LICENSE-MIT` / `LICENSE-APACHE` | デュアルライセンスの全文 2 ファイル | ripgrep / fd / bat / uv が同じ配置 |
| `CHANGELOG.md` | Keep a Changelog 形式（release-plz で自動更新） | |
| `CONTRIBUTING.md` | ビルド・テスト、コミット規約、**ルール追加ガイド**（`rules/builtin.toml` の書き方、safety の判断基準、テストの書き方） | |
| `SECURITY.md` | 報告窓口（GitHub の Private vulnerability reporting）、対象バージョン。**削除系のバグは脆弱性扱い**と明記 | ripgrep / fd / bat / uv / starship が設置 |
| `CODE_OF_CONDUCT.md` | Contributor Covenant 3.0（2025-07 公開）または 2.1 | starship / eza が設置 |
| `.github/ISSUE_TEMPLATE/*.yml` | bug（`roomy --version` と `roomy doctor` の出力欄）、rule request（パス・再生成可否・根拠 URL）、`config.yml` で質問は Discussions へ | |
| `.github/PULL_REQUEST_TEMPLATE.md` | テスト済みか、削除系の変更か、ルールの根拠 | |
| `.github/dependabot.yml`, `.github/workflows/{ci,release}.yml` | §5 | |

- デュアルライセンスの README 末尾には、Rust API Guidelines（C-PERMISSIVE）にある定型文を使う
  （"Licensed under either of Apache License, Version 2.0 or MIT license at your option." と、貢献物も同条件になる旨の一文）。
- `Cargo.toml` の `readme = "README.md"` は、README が無いと `cargo package` / `cargo publish` が失敗する。crates.io に出さないなら `publish = false` も付ける。

### 6.2 README の構成（例）

1. 1 行説明 ＋ スクリーンショット（`status` と `report`）
2. 何が違うか（開発者特有の溜まり方、根拠の表示、増加検出）。DESIGN §1 の要約
3. **安全性の約束**: 自動では消さない、削除直前に再検証、`$HOME` 外と保護フォルダは拒否、監査ログ、テレメトリなし
4. インストール
   - `brew install masa-tichita/tap/roomy`（Homebrew 6 以降は tap の信頼が必要。完全修飾名で install すると、その formula だけ信頼される [確認済: docs.brew.sh/Tap-Trust]）
   - `cargo install --git …`
   - Releases から直接（quarantine の注意）
5. クイックスタート: `roomy status` → `roomy scan` → `roomy clean 1 2 --dry-run` → `brew services start roomy` → `roomy doctor`
6. フルディスクアクセス（なぜ要るか・無くても何が動くか・アップグレード後の付け直し）
7. 設定（`roomy config init`、主なキー、ルールの追加・無効化・上書き）
8. 機械向け: `--json` の schema 方針、終了コード表
9. アンインストール（`brew services stop` / `roomy service uninstall --purge`）
10. ライセンス

- バッジは 3〜4 個まで: CI、ライセンス、最新リリース（`img.shields.io/github/v/release/...`）、（公開するなら）crates.io。

---

## 7. macOS 固有

### 7.1 launchd

[確認済: launchd.plist(5)]

| キー | 挙動 | roomy |
|---|---|---|
| `StartInterval` | N 秒ごとに起動。**スリープ中に来た回は飛ばされる**。実行中に来た回もスキップ | 使用中（15 分）。agent が毎回「最終スキャンから 24 時間経ったか」を見るので取りこぼしは問題にならない。**このままでよい** |
| `StartCalendarInterval` | cron 風の時刻指定。スリープ中に過ぎた回は**起床時にまとめて 1 回**実行 | 「毎日 9 時に要約通知」のような用途なら候補 |
| `ProcessType` | `Background` = ユーザーが直接頼んでいない仕事。資源制限がかかる。`Standard` は指定なしと同じ。`Interactive` は制限なし | `Background` で正しい |
| `LowPriorityIO` / `Nice` | I/O の低優先度、nice 値 | 設定済み。さらにプロセス内で `setiopolicy_np(IOPOL_THROTTLE)`（重複だが害はない） |
| `RunAtLoad` | 読み込み時に 1 回起動。man は「起動・ログイン時の負荷になるので避けるべき」と書いている | 現在 `true`。`service install` 直後の動作確認には便利。ログイン直後の負荷を避けるなら `false` にし、install 時に `launchctl kickstart` で 1 回起動する |
| `ThrottleInterval` | 既定 10 秒 | 既定でよい |
| `KeepAlive` | 常駐させる | 定期実行なので使わない（正しい） |

- `launchctl bootstrap gui/$UID` / `bootout` / `print` を使っており、古い `load`/`unload` を避けている点も良い。

### 7.2 Homebrew: formula か cask か

- **CLI は formula。** 自前の tap（`masa-tichita/homebrew-tap`）にビルド済みバイナリを入れる formula を置くのは一般的で、dist もこの形の formula を生成する。
- cask は .app や pkg 向け。未署名の cask は非推奨になりつつある（§5.6）。
- Homebrew 7 の変更点 [確認済: 7.0.0 告知]:
  - formula の操作はサンドボックス内で行われ、ホームディレクトリを読めない
  - `post_install` は非推奨（third-party tap は 2027-12-11 まで警告）
  - **formula から LaunchAgent を作ったり状態ファイルを書いたりしない。** `service do` に任せる
- tap の信頼（Homebrew 6 以降）: README の導入手順は完全修飾名 `brew install masa-tichita/tap/roomy` にする。

formula の例（tap に手で置き、リリースのたびに url/sha256/version を CI で書き換える）:

```ruby
class Roomy < Formula
  desc "Keeps your Mac roomy: finds what is safe to delete, warns before storage runs out"
  homepage "https://github.com/masa-tichita/roomy"
  url "https://github.com/masa-tichita/roomy/releases/download/v0.2.0/roomy-v0.2.0-universal-apple-darwin.tar.gz"
  sha256 "…"
  license any_of: ["MIT", "Apache-2.0"]
  depends_on :macos

  def install
    bin.install "roomy"
    generate_completions_from_executable(bin/"roomy", "completions")
  end

  service do
    run [opt_bin/"roomy", "agent"]
    run_type :interval
    interval 900                 # 秒。StartInterval になる
    process_type :background
    nice 10
    environment_variables PATH: std_service_path_env
    log_path var/"log/roomy.log"
    error_log_path var/"log/roomy.log"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/roomy --version")
  end
end
```

- `run_type :interval` ＋ `interval` は plist の `StartInterval`（秒）に、`process_type` は `ProcessType` に変換される
  [確認済: Homebrew の `service.rb`。`base[:StartInterval] = @interval`、`base[:ProcessType] = @process_type.to_s.capitalize`]。
- 間隔は formula に固定される。ユーザーの `check_interval_min` を反映したいなら、`brew services` 用の上書きファイル（Homebrew 7 の `$HOMEBREW_USER_CONFIG_HOME/services/<formula>.env`、ただし環境変数の上書き）か、agent 側で間引く。
  「agent は 15 分ごとに起動し、設定の間隔に満たなければ何もせず終了する」実装が単純。

### 7.3 brew services のラベル（要対応）

- [確認済: 手元の Homebrew 7.0.9 の `Library/Homebrew/service.rb`]
  - 既定の plist 名は `sh.brew.<formula>`（`canonical_plist_name`）
  - 旧名 `homebrew.mxcl.<formula>`（`legacy_plist_name`）は互換のため認識される
  - 7.0.0 の告知では「新規・再起動したサービスは `sh.brew.<formula>`」
- roomy の `app::BREW_LABEL = "homebrew.mxcl.roomy"` だけでは、新しい環境での二重登録の検出（`service install` の拒否、`doctor` の警告、`status_line`）が効かない。
- **対応**: `BREW_LABELS: [&str; 2] = ["sh.brew.roomy", "homebrew.mxcl.roomy"]` のどちらかが読み込まれていれば brew 管理とみなす。

---

## 8. ギャップリスト（優先度つき）

### 8.1 すでに良くできている点

- 名前に依存する値を `app.rs` の 1 か所に集約（改名が容易）。ラベルは逆 DNS 形式
- 安全装置: パスガード、削除直前の再検証、監査ログ、`flock` による同時実行防止、外部コマンドのタイムアウト、`GIT_OPTIONAL_LOCKS`
- 状態ファイルを 0600 / 0700 で保存、テレメトリなし
- 全モジュールに `//!` の説明。設計書（DESIGN.md）が詳しい
- 既定の clippy で警告 0、テスト 69 件が成功
- e2e は実バイナリを `HOME` 差し替えで実行し、プロセス内で環境変数を書き換えない（並列テストでも安全）
- i18n: キー網羅・プレースホルダ一致・コード中のキー存在をテストで保証。保存はキー、表示時に翻訳
- `status` / `report` の JSON に `schema` がある。`agent` は隠しサブコマンド
- LaunchAgent: `bootstrap`/`bootout` を使用、`ProcessType=Background`、`LowPriorityIO`、XML エスケープ、Homebrew の `opt` パスへの正規化、開発ビルドからの install を拒否
- `NO_COLOR` と tty の判定、`unicode-width` による全角の桁揃え
- 設定は XDG、状態は Application Support。未知の設定キーは警告にとどめる
- release プロファイルが `lto = "thin"`, `strip = true`

### 8.2 今すぐ（v0.1 のタグ・公開前）

| # | 内容 | 対象ファイル | 工数 |
|---|---|---|---|
| P0-1 | `rust-version` を 1.88 に（clap 4.6 で 1.85、ratatui 0.30.2 で 1.88）。`edition = "2024"` へ移行し、`set_var` を `Command::env` に、`extern` を `unsafe extern` に | `Cargo.toml`, `src/main.rs`, `src/sys.rs` | 小 |
| P0-2 | CI（fmt / clippy `-D warnings` / test / MSRV / cargo-deny）を追加し、`cargo fmt` を一度かける | `.github/workflows/ci.yml`, `deny.toml` | 小 |
| P0-3 | `[lints]` テーブルと `clippy.toml`。`unsafe` 7 か所に `// SAFETY:`、`getuid` を 1 関数に集約 | `Cargo.toml`, `clippy.toml`, `src/sys.rs`, `src/service.rs` | 小〜中 |
| P0-4 | 閾値超過の終了コードを 2 から 3 へ変更し、表を README と `--help` に記載 | `src/main.rs`, `docs/DESIGN.md` | 小 |
| P0-5 | brew services のラベル `sh.brew.roomy` と `homebrew.mxcl.roomy` の両方を検出 | `src/app.rs`, `src/service.rs`, `src/doctor.rs` | 小 |
| P0-6 | `--json` の契約: `top`/`rules` を `{"schema":1,…}` で包む、JSON を言語非依存に、互換規則を文書化 | `src/output.rs`, `tests/e2e.rs`（insta） | 中 |
| P0-7 | `sys::try_lock` に `#[must_use]`。非テストの `unwrap()`（`parent()`, `to_string_pretty`）を `?` に | `src/sys.rs`, `src/main.rs`, `src/service.rs`, `src/output.rs` | 小 |
| P0-8 | README（英・日）、`LICENSE-MIT`、`LICENSE-APACHE`、`CHANGELOG.md`、`SECURITY.md`、`CONTRIBUTING.md`（ルール追加ガイド込み） | リポジトリ直下 | 中 |
| P0-9 | 直接依存の整理: `objc2` が不要なら削除、ratatui の feature を絞る（cargo-machete / cargo-shear で確認） | `Cargo.toml` | 小 |

### 8.3 v0.2（配布開始時）まで

| # | 内容 |
|---|---|
| P1-1 | release workflow: arm64 ＋ x86_64 をクロスビルド → `lipo` → tarball ＋ sha256 → GitHub Release → attestation。`MACOSX_DEPLOYMENT_TARGET=13.0` |
| P1-2 | tap に手書きの formula（`service do` の `run_type :interval`、`process_type :background`、補完、`test do`）。リリースのたびに CI で url/sha256 を更新 |
| P1-3 | release-plz（`publish=false`, `git_only=true`, `git_release_enable=false`）＋ Conventional Commits ＋ Keep a Changelog |
| P1-4 | `roomy completions <shell>` / `roomy man`（clap_complete 4.6.11 / clap_mangen 0.3.3） |
| P1-5 | 色: `anstream` に移行、`--color auto|always|never`、`CLICOLOR_FORCE` 対応、`NO_COLOR=""` は無視 |
| P1-6 | insta で `report --json` / `status --json` / `--help` のスナップショット |
| P1-7 | `history --json`、`doctor --json`。`long_version`（コミット・ターゲット） |
| P1-8 | `clean` の失敗行とガード拒否を stderr へ |
| P1-9 | Dependabot（cargo ＋ github-actions）、action の SHA 固定、Issue/PR テンプレート、CODE_OF_CONDUCT |
| P1-10 | Intel ランナー（`macos-15-intel`）で週 1 テスト。2027 年秋のランナー廃止で外す |
| P1-11 | README に FDA の注意（アップグレードで外れる、付け直し手順）と「バックグラウンド項目」通知の説明 |

### 8.4 後でよい（v0.3 以降）

| # | 内容 |
|---|---|
| P2-1 | Developer ID 署名 ＋ hardened runtime ＋ 公証（TCC の許可がバージョンを跨いで維持される）。CI で証明書を扱うなら rcodesign も選択肢 |
| P2-2 | lib/bin 分割（内部ロジックの結合テストやベンチが増えたら）。just の構成を参考に |
| P2-3 | newtype / enum（`Pressure`, `Bytes`, `CandidateId`） |
| P2-4 | ログ基盤（`log` ＋ `env_logger` か `tracing`）。`-v/-q`、`ROOMY_LOG` |
| P2-5 | `cargo-auditable`、SBOM |
| P2-6 | 言語検出を `defaults` の子プロセスから `NSLocale` に置き換え |
| P2-7 | trycmd で README のコマンド例を検証、proptest で guard を検証 |
| P2-8 | `state::load` の「無い」と「壊れている」を区別（`doctor` で破損を報告） |

---

## 9. 未検証・要確認の事項

- GitHub の Intel ランナー（`macos-15-intel` / `macos-26-intel`）の正確な廃止日（資料により「2027 年 8 月」「2027 年秋」）
- 単体の Mach-O に公証チケットを staple できないこと（Apple の notarization ドキュメントで要確認）
- Homebrew formula 経由のダウンロードに quarantine が付かないこと（広く知られているが一次資料は未確認）
- LaunchAgent 追加時の通知に出る名前が、未署名のときにどう表示されるか
- Rust の macOS ターゲットの既定デプロイメントターゲット（aarch64 11.0 / x86_64 10.12）の現行値
- lipo で結合した後も各スライスの ad-hoc 署名が有効か（`codesign -v` で確認する）
- objc2-foundation 0.3 の `NSLocale` の API 名と feature 名
- cargo-deny 0.20 の `deny.toml` の細かなキー名（`cargo deny init` の雛形に従う）
- Apple Developer Program の日本での価格

---

## 10. 出典

**Rust 公式**
- Rust 2024 Edition Guide: https://doc.rust-lang.org/edition-guide/rust-2024/index.html
  - newly unsafe functions（`set_var`）: https://doc.rust-lang.org/edition-guide/rust-2024/newly-unsafe-functions.html
  - unsafe extern: https://doc.rust-lang.org/edition-guide/rust-2024/unsafe-extern.html
  - unsafe_op_in_unsafe_fn: https://doc.rust-lang.org/edition-guide/rust-2024/unsafe-op-in-unsafe-fn.html
  - Cargo resolver v3: https://doc.rust-lang.org/edition-guide/rust-2024/cargo-resolver.html
  - rustfmt style edition: https://doc.rust-lang.org/edition-guide/rust-2024/rustfmt-style-edition.html
- Cargo Book `[lints]`: https://doc.rust-lang.org/cargo/reference/manifest.html#the-lints-section
- Cargo Book `rust-version`: https://doc.rust-lang.org/cargo/reference/rust-version.html
- Clippy の設定: https://doc.rust-lang.org/clippy/configuration.html ／ lint 一覧: https://doc.rust-lang.org/clippy/lints.html
- rustfmt: https://rust-lang.github.io/rustfmt/
- Rust API Guidelines チェックリスト: https://rust-lang.github.io/api-guidelines/checklist.html
- Rust CLI Book（テスト）: https://rust-cli.github.io/book/tutorial/testing.html
- Rust CLI Book（終了コード）: https://rust-cli.github.io/book/in-depth/exit-code.html
- Rust CLI Book（人間向け出力・機械向け出力・ドキュメント）:
  - https://rust-cli.github.io/book/in-depth/human-communication.html
  - https://rust-cli.github.io/book/in-depth/machine-communication.html
  - https://rust-cli.github.io/book/in-depth/docs.html
- x86_64-apple-darwin の Tier 2 への格下げ: https://blog.rust-lang.org/2025/08/19/demoting-x86-64-apple-darwin-to-tier-2-with-host-tools ／ Rust 1.90.0: https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/
- Platform support: https://doc.rust-lang.org/rustc/platform-support.html
- 現行 stable の確認: https://static.rust-lang.org/dist/channel-rust-stable.toml

**ツール・crate**
- dist 本: https://axodotdev.github.io/cargo-dist/book/ ／ Homebrew の章: https://axodotdev.github.io/cargo-dist/book/installers/homebrew.html ／ 設定: https://axodotdev.github.io/cargo-dist/book/reference/config.html
- dist の Releases: https://github.com/axodotdev/cargo-dist/releases ／ Universal 非対応の Issue #77: https://github.com/axodotdev/cargo-dist/issues/77 ／ 署名と hardened runtime の Issue #1534: https://github.com/axodotdev/cargo-dist/issues/1534 ／ formula テンプレート: https://github.com/axodotdev/cargo-dist/blob/main/cargo-dist/templates/installer/homebrew.rb.j2 ／ 署名の実装: https://github.com/axodotdev/cargo-dist/blob/main/cargo-dist/src/sign/macos.rs
- release-plz 設定: https://release-plz.dev/docs/config
- cargo-deny: https://embarkstudios.github.io/cargo-deny/
- Swatinem/rust-cache: https://github.com/Swatinem/rust-cache
- Dependabot の設定: https://docs.github.com/en/code-security/dependabot/working-with-dependabot/dependabot-options-reference
- GitHub-hosted runners: https://docs.github.com/en/actions/reference/runners/github-hosted-runners
- Artifact attestations: https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations/using-artifact-attestations-to-establish-provenance-for-builds
- clap_complete: https://docs.rs/clap_complete ／ clap_mangen: https://docs.rs/clap_mangen ／ anstream: https://docs.rs/anstream
- assert_cmd: https://docs.rs/assert_cmd ／ assert_fs: https://docs.rs/assert_fs ／ insta: https://insta.rs/ ／ trycmd: https://docs.rs/trycmd
- rust-i18n: https://docs.rs/rust-i18n ／ Fluent: https://projectfluent.org/
- apple-codesign（rcodesign）: https://github.com/indygreg/apple-platform-rs
- バージョン情報: crates.io API（`https://crates.io/api/v1/crates/<name>`）、各 action の GitHub Releases（2026-10-10 取得）

**参考にした CLI のリポジトリ**
- ripgrep: https://github.com/BurntSushi/ripgrep ／ fd: https://github.com/sharkdp/fd ／ bat: https://github.com/sharkdp/bat
- just: https://github.com/casey/just ／ zoxide: https://github.com/ajeetdsouza/zoxide ／ uv: https://github.com/astral-sh/uv
- starship: https://github.com/starship/starship ／ eza: https://github.com/eza-community/eza
- dust: https://github.com/bootandy/dust ／ dua-cli: https://github.com/Byron/dua-cli

**規約・仕様**
- NO_COLOR: https://no-color.org/ ／ CLICOLOR: https://bixense.com/clicolors/
- Keep a Changelog 1.1.0: https://keepachangelog.com/en/1.1.0/ ／ Conventional Commits 1.0.0: https://www.conventionalcommits.org/en/v1.0.0/ ／ SemVer: https://semver.org/
- Contributor Covenant 3.0: https://ethicalsource.dev/blog/contributor-covenant-3

**Homebrew**
- Formula Cookbook（`service do`）: https://docs.brew.sh/Formula-Cookbook
- Tap Trust: https://docs.brew.sh/Tap-Trust ／ Taps: https://docs.brew.sh/Taps ／ tap の作り方: https://docs.brew.sh/How-to-Create-and-Maintain-a-Tap ／ Support Tiers: https://docs.brew.sh/Support-Tiers
- Homebrew 5.0.0: https://brew.sh/2025/11/12/homebrew-5.0.0/ ／ 6.0.0: https://brew.sh/2026/06/11/homebrew-6.0.0/ ／ 7.0.0: https://brew.sh/2026/09/13/homebrew-7.0.0/
- service.rb（ラベル・`StartInterval`・`ProcessType` の変換）: https://github.com/Homebrew/brew/blob/master/Library/Homebrew/service.rb

**Apple / macOS**
- launchd.plist(5): https://keith.github.io/xcode-man-pages/launchd.plist.5.html
- TN3127 Inside Code Signing: Requirements: https://developer.apple.com/documentation/technotes/tn3127-inside-code-signing-requirements
- ad-hoc 署名で TCC の許可が外れる件（Apple Developer Forums）: https://developer.apple.com/forums/thread/795739
- Notarizing macOS software: https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution
- Ventura のログイン項目・バックグラウンド項目（Eclectic Light）: https://eclecticlight.co/2023/02/16/how-ventura-is-changing-login-and-background-items
