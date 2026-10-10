# roomy 設計書

> 名前は `roomy`（「広々とした」）。コード上の名前は
> `Cargo.toml` の crate 名と `src/app.rs` の定数 1 か所に集め、パス・launchd ラベル・
> 環境変数名はすべてそこから作る。改名は 1 か所の変更で済むようにする。

## 1. 目的

開発者の Mac で起きる「気づいたらストレージが枯渇している」「メモリが逼迫している」を
早めに知らせ、**何を消せば安全にどれだけ空くか**を根拠付きで示す CLI。

既存ツール（Stats、DaisyDisk、CleanMyMac、Mole など）との違い:

- **開発者特有の溜まり方を理解する**: git worktree（push 済みか・未コミットか・使用中か）、
  Xcode のテスト用シミュレータ複製、ツールの旧バージョン、エディタの旧拡張機能、Docker など。
- **増加の検出**: 1 回の走査でフォルダごとの使用量を記録し、前回との差分から
  「どこが急に増えたか」を全フォルダについて出す。
- **消す判断の根拠を必ず表示する**（「push 済み・未コミット 0・最終操作 12 日前」など）。

対象外（やらないこと）: システム領域の掃除、sudo を要する操作、自動削除、外部送信。

## 2. 設計原則

1. **自動では消さない。** 削除はユーザーが `clean` で明示したときだけ。確認と dry-run あり。
2. **消す直前に再検証する。** レポート作成後に状況が変わっていたら（worktree に変更が入った、
   アプリが起動した等）その候補は消さない。
3. **知らない環境で壊れない。** git / docker / Xcode などが無ければその検出器は黙ってスキップ。
   Apple Silicon / Intel 両対応、Homebrew のプレフィックス差を吸収。
4. **個人の事情は設定へ。** 既定値は誰の Mac でも安全な保守的な値にし、
   個別の事情（特定アプリを残す、Cowork は使わない等）は設定とユーザールールで表す。
5. **権限ダイアログを乱発しない。** バックグラウンド実行では TCC 保護領域に触れない（§7）。
6. **データは手元から出さない。** テレメトリなし。状態ファイルは 0600 / ディレクトリは 0700。

## 3. コマンド

| コマンド | 内容 |
|---|---|
| `status` | 空き容量（df 相当 / Finder 相当）、メモリ逼迫度・空き率・スワップ、メモリ上位アプリ、最終スキャン要約 |
| `scan` | 走査 → 検出 → レポート保存 → 表示。`--notify` で要約を通知、`--json` |
| `report` | 最後のスキャン結果を表示（再走査しない）。`--verbose` で対象外 worktree の理由など |
| `clean <番号/ID...>` | 候補を削除。`--dry-run`、`--yes`、`--trash`。実行前に再検証、実行後に実際に空いた量 |
| `top [PATH]` | 最新スナップショットからフォルダの大きい順と前回比（再走査不要の du 代わり） |
| `history` | 空き容量の推移（日ごとの最小値） |
| `explain <id>` | 候補 1 件の詳細（根拠・注意・削除対象のパスやコマンド） |
| `guide` | AI エージェント向けの手順。Claude Code のスキルや AGENTS.md 用の文面としても出力 |
| `mcp` | MCP サーバー（stdio）。CLI の `--json` と同じ結果をツールとして提供（§14） |
| `tick` | launchd から定期実行される入口（非公開）。軽いチェック＋必要なら日次スキャン（§6） |
| `service install / uninstall / status` | LaunchAgent の登録・解除・状態 |
| `doctor` | 権限（FDA）、使える外部ツール、サービス状態、設定の妥当性、通知手段を診断 |
| `config path / show / init / validate` | 設定ファイル |
| `rules` | 有効なルール一覧（組み込み＋ユーザー定義、無効化状態） |

全コマンド `--json` 対応。JSON は `{"schema": 1, ...}` でバージョン付けし、
Claude Code などのエージェントからも扱えるようにする。終了コードも規定（0 正常、
1 エラー、2 引数の誤り（clap）、3 閾値超過あり（`status --exit-code` 指定時））。

## 4. 検出の仕組み

### 4.1 走査（walk）

- `rayon` で並列にディレクトリを読み、フォルダごとの割り当てサイズ（du 相当）を 1 パスで集計。
- シンボリックリンクは辿らない。別デバイス（NFS マウント等）に入らない。ハードリンクは 1 回だけ数える。
- 記録するのは `snapshot_min_mb` 以上・深さ `snapshot_max_depth` 以内のフォルダ（数千件規模）。
- 低優先度で動かす: `setiopolicy_np(IOPOL_THROTTLE)` ＋ launchd の `ProcessType=Background`。
- 読めなかったフォルダ数を記録し、権限不足が原因なら `doctor` へ誘導。
- **APFS クローン**: 割り当てサイズはクローン共有分も数えるため、XCTestDevices のように
  実際に空く量が表示より小さくなる。削除候補に限り `getattrlist(ATTR_CMNEXT_PRIVATESIZE)` で
  「消したら実際に空く量」を測る（**要検証**: 対応 OS と速度。使えなければ注記のみ）。

### 4.2 増加検出

前回スナップショットとの差分で `growth_alert_gb` 以上増えたフォルダを出す。
増加の 7 割以上が子フォルダで説明できる親は省き、原因に近い階層を表示する。
走査ルートや除外設定が前回と違う場合は比較しない。

### 4.3 ルール（データ駆動）

単純な「この場所はこう扱う」はコードではなくデータ（TOML）で定義する。
組み込みルールは `rules/*.toml` をバイナリに埋め込み、ユーザーは設定の `[[rules]]` で追加、
`disabled_rules` で無効化、`[rule_overrides.<id>]` で判定を上書きできる。

```toml
[[rules]]
id = "xcode.derived-data"
paths = ["~/Library/Developer/Xcode/DerivedData"]
target = "children"            # self | children
safety = "safe"                # safe | review
action = "delete"              # delete | trash | command | none
command = []                   # action = "command" のとき（例: ["npm", "cache", "clean", "--force"]）
requires = []                  # 必要な外部コマンド。無ければルールごとスキップ
skip_if_running = ["Xcode"]    # 起動中なら review に格下げ
idle_marker = ""               # このファイルの更新が idle_days 以上前なら safe（例: Cowork VM）
idle_days = 0
min_mb = 100
note = "ビルド中間生成物。次回ビルドで再生成"   # 組み込みは i18n キー
```

### 4.4 コード検出器（データでは表せないもの）

| 検出器 | 判定 |
|---|---|
| worktree | 下記 4.5 |
| versioned-dirs | バージョンごとに増えるフォルダ（Codex の releases 等）。最新 N 件・`current` の参照先・実行中のものを残す |
| editor-obsolete | VS Code 系（Code / Cursor / Windsurf / Antigravity 等）の `extensions/.obsolete` に載った旧拡張 |
| xcode-device-support | `* DeviceSupport` のうち各デバイスの最新以外 |
| xcode-sims | `simctl` で利用不可になったシミュレータ |
| docker | デーモンが**既に動いている場合だけ**問い合わせる（CLI 実行で Docker/OrbStack を起動させない） |
| apps | 一定サイズ以上で最終使用（Spotlight の `kMDItemLastUsedDate`）が古いアプリ。削除はせずアンインストーラ案内のみ |
| downloads | 古いファイル。review・ゴミ箱のみ（Downloads は TCC 保護。§7） |

検出器は設定 `disabled_detectors`（例: `["docker", "apps"]`）で個別に無効化できる。
無効化された検出器の候補は `clean` の再検証で拒否する。
apps は走査したフォルダ（`scan.roots` 配下で `scan.exclude` 外）だけを見る。サイズは
スナップショットから取り、記録されていない（`snapshot_min_mb` 未満の）ものは測り直さない。

### 4.5 worktree 検出

- 探索ルート: 設定 `worktree_roots`（既定は `~/dev ~/src ~/code ~/projects ~/Developer ~/work ~/repos` のうち存在するもの、
  および `~/.codex/worktrees`）。深さ 5 まで、`node_modules` 等は降りない。
- `.git` が**ファイル**で `gitdir:` が `/worktrees/` を含むものを worktree とみなす（submodule と区別）。
- 判定項目（すべて表示に出す）:
  - 未コミット変更（`git status --porcelain`、`GIT_OPTIONAL_LOCKS=0` で index を書き換えない）
  - push 済みか（upstream との差、upstream 無しなら HEAD がどこかのリモートブランチに含まれるか）
  - 既定ブランチへの取り込み有無（squash merge は検出できない旨を注記）
  - 最終操作（HEAD の reflog 時刻とコミット時刻の新しい方）
  - 使用中か（自分のプロセスの cwd がその中にあるか。`lsof -d cwd`。worktree が見つかったときだけ実行）
  - **Git 管理外の固有ファイル**: `--ignored` の結果から再生成可能なもの（`node_modules`、`.venv`、
    `target`、`.next` 等）を除いたもの。`.env` やベンチ結果のような「消えたら戻らない」ものを列挙する
- 分類: 未コミット・未 push・使用中・最近使用 → 対象外（理由を `report --verbose` で表示）。
  それ以外で固有ファイル無し → safe、有り → review。
- 削除は `git worktree remove`（`--force` は使わない）。ブランチは消さない。
- Orca など外部ツール管理下のフォルダは設定 `worktrees.managed` で注記（「ツール側から消すのを推奨」）。

## 5. 削除の安全装置

1. **再検証**: `clean` は保存済みレポートを信用せず、対象の検出器で条件を再評価する。
   変わっていれば中止してその旨を表示。レポートが古い（既定 3 日）場合は再スキャンを促す。
2. **パスガード**: 削除対象は `$HOME` 配下のみ。以下は常に拒否:
   `~` 自体、`~/Desktop`、`~/Documents`、`~/Pictures`、`~/Movies`、`~/Music`、`~/.ssh`、`~/.gnupg`、
   `~/Library/Keychains`、`~/Library/Mail`、`~/Library/Messages`、`~/Library/Mobile Documents`、
   `~/Library/CloudStorage`、およびそれらの祖先。パスの途中にシンボリックリンクがあれば拒否。
3. **削除方法の使い分け**: 再生成できるもの（キャッシュ等）は完全削除（ゴミ箱では容量が空かないため）。
   ユーザーデータ寄りのもの（Downloads 等）はゴミ箱（`NSFileManager.trashItem`）。`--trash` で全件ゴミ箱。
4. **外部コマンド優先**: ツールが自前の掃除コマンドを持つ場合はそれを使う（`npm cache clean`、
   `uv cache clean`、`brew cleanup`、`git worktree remove`、`xcrun simctl delete unavailable` 等）。
5. **監査ログ**: 実行した操作・対象・見込み量・実際に空いた量を `audit.jsonl` に追記。
6. Rust の `remove_dir_all` はシンボリックリンクを辿らない（CVE-2022-21658 修正済みの版を MSRV にする）。

## 6. 定期実行（agent）

LaunchAgent は 1 つだけ（`StartInterval` = `check_interval_min`、既定 15 分）。
`agent` は毎回:

1. 空き容量・メモリ逼迫度・スワップを測り、`history.jsonl` に 1 行追記。
2. 閾値判定して通知（下表）。通知ごとにクールダウンを持ち、連投しない。
3. 最終スキャンから 24 時間以上経っていて、AC 電源接続中（またはバッテリー 50% 以上）なら日次スキャン。
   スリープ明けでも取りこぼさない。

| 通知 | 条件（既定） | クールダウン |
|---|---|---|
| 空き容量 危険 | < 25 GB | 1 時間 |
| 空き容量 警告 | < 50 GB | 12 時間 |
| 空き容量 急減 | 前回チェックから 10 GB 以上減 | 1 時間 |
| メモリ逼迫 | 逼迫度 警告 / 危険 | 2 時間 / 30 分 |
| スワップ | > 4 GB | 6 時間 |
| 日次スキャン | 増加検出あり、または安全に空く量 ≥ 10 GB、または空き < 警告 | 1 日 |

- 通知手段: `osascript display notification`（追加インストール不要）。PATH に `terminal-notifier` があればそれを使う。
- 同時実行防止: 状態ディレクトリのロックファイルに `flock`。
- ログ: `logs/agent.log`、1 MB でローテーション。

Homebrew 経由なら formula の `service`（`run_type :interval`）で同じ LaunchAgent を管理できる。
`service install` は Homebrew 以外（cargo install 等）の導入向け。両方が登録されていたら `doctor` が警告する。

## 7. macOS の権限（TCC / フルディスクアクセス）

- Desktop / Documents / Downloads / 他アプリのコンテナ等は TCC で保護されており、
  バックグラウンドのバイナリが触ると許可ダイアログが出る。
- FDA の有無を `~/Library/Application Support/com.apple.TCC/TCC.db` が読めるかで判定。
- **FDA が無い場合、agent からのスキャンは保護領域を走査しない**（ダイアログを出さない）。
  対話実行（`scan` を端末から）は端末アプリの権限で動くので走査する。
- `doctor` が FDA の状態と付与手順を表示。
- 注意: 未署名（ad-hoc 署名）のバイナリは、ビルドし直すと FDA の許可が外れる。
  リリース版は Developer ID 署名＋公証を目標（Apple Developer Program が必要。初期は未署名で、
  FDA 無しでも主要機能が動くことを優先）。

## 8. 表示と多言語

- 既定は英語。`LC_ALL` / `LC_MESSAGES` / `LANG`、無ければ macOS の優先言語（launchd では LANG が無いため）で判定。
  設定 `language = "auto" | "en" | "ja"` で固定可。
- メッセージは `locales/en.toml` / `locales/ja.toml`。全キーが両方にあることをテストで保証。
- 端末なら色付き（`NO_COLOR` を尊重）。日本語の桁揃えは表示幅（`unicode-width`）で行う。
- 容量は Finder と同じ 10 進（GB）。

## 9. ファイル配置

| 種類 | 場所 |
|---|---|
| 設定 | `~/.config/roomy/config.toml`（`XDG_CONFIG_HOME` を尊重） |
| 状態 | `~/Library/Application Support/roomy/`（report.json、snapshots/、history.jsonl、audit.jsonl、check.json、logs/） |
| LaunchAgent | `~/Library/LaunchAgents/io.github.masa-tichita.roomy.agent.plist` |

スナップショットは 30 件保持。`uninstall --purge` で状態も削除。
設定は `config_version` を持ち、未知のキーは警告（エラーにはしない）。

## 10. 品質

- 単体テスト: 外部コマンド出力のパーサ（docker の容量表記、mdls の日時、git porcelain、
  `.obsolete`）、パスガード、ルール評価、増加検出、i18n キー網羅。
- 統合テスト: 一時ディレクトリでの走査（ハードリンク・シンボリックリンク）、一時 git リポジトリで
  worktree の各状態（未コミット / 未 push / 固有ファイル有り / 使用中）を作って分類を確認。
  削除系は一時ディレクトリだけを対象にする。
- CI（GitHub Actions）: Apple Silicon ランナーで fmt / clippy / test。Intel ランナーが使えれば併用。
- `cargo-deny` でライセンス・脆弱性確認。MSRV を明記。
- ライセンス: MIT OR Apache-2.0。README（英語＋日本語）、CHANGELOG、CONTRIBUTING、ルール追加ガイド。

## 11. 配布

- GitHub Releases: タグで CI が aarch64 / x86_64 をビルドし `lipo` で Universal バイナリ化。
- Homebrew tap（`masa-tichita/homebrew-tap`）: formula から Releases のバイナリを入れ、`service` で agent を管理。
- 新しい Mac での導入: `brew install masa-tichita/tap/roomy` → `brew services start roomy` → `roomy doctor`。
- 個人設定（`~/.config/roomy/config.toml`）は dotfiles で持ち運べる。

## 12. 対応環境

- macOS 13 以降（Apple Silicon / Intel）。動作確認は手元の macOS 26。
- 外部ツールはすべて任意: git、docker、xcrun、npm、uv、pnpm、brew、mdls、lsof（mdls / lsof は標準搭載）。

## 13. 段階的な実装

| 段階 | 内容 |
|---|---|
| v0.1 | 走査・増加検出、ルール（組み込み＋ユーザー）、コード検出器（4.4 のすべて）、`status / scan / report / clean / top / agent / service / doctor / config / rules`、再検証・パスガード・監査ログ、en/ja、JSON、テスト、CI。`cargo install --git` で自分の Mac に導入 |
| v0.2 | Releases（Universal バイナリ）＋ Homebrew tap、`history`、メモリを食い続けるアプリの検知、APFS 実効サイズ |
| v0.3 | 署名・公証、名前確定・公開、ルールのコミュニティ追加 |
| 以降 | メニューバー表示（SwiftBar プラグイン出力）、外部ボリューム対応 |

## 14. AI エージェント対応（主要な使い方）

エージェントが即興の `du` / `rm` の代わりに roomy を呼ぶことで、結果が決定的になり、トークンも安全性も改善する。

- **要約優先の出力**: `report --json` は候補ごとに id・容量・判定・根拠・注意・操作の種類だけ（パス一覧なし）。
  詳細は `explain <id> --json`。ターミナル以外には改行なしの JSON を出す。実測で全量 約 90 KB → 要約 約 10 KB。
- **言語非依存**: JSON は常に英語。`--json` と MCP では UI 言語の設定を無視する。エラーも JSON で返す。
- **削除の契約**: `clean <id> --json` はドライラン（計画と拒否理由）、`--yes` を付けて初めて削除。
- **無人実行の歯止め**（`--yes`・`--json`・MCP）: 「要確認」は `--allow-review` が必要。
  1 回の上限 `[clean] max_unattended_gb`（既定 50 GB）。監査ログに `unattended` を記録。
- **説明書の同梱**: `roomy guide`（4 KB 未満をテストで保証）、`--format skill`（Claude Code スキル）、
  `--format agents-md`、`--install-claude-skill`。
- **MCP サーバー**: 改行区切り JSON-RPC 2.0。`initialize` でクライアントのプロトコル版に合わせる。
  ツールは status / report / scan / explain / top / history / doctor / plan_clean / clean。
  `readOnlyHint` / `destructiveHint` の注釈付き。`--read-only` で clean を出さない。
  scan は常駐サービスと同じ扱い（フルディスクアクセスが無ければ保護フォルダを走査しない）で、ホストアプリ名義の許可ダイアログを出さない。

## 15. 未決事項

- 名前
- `ATTR_CMNEXT_PRIVATESIZE` の実用性（対応 OS・速度）
- Developer ID 署名をするか（費用）
- Intel の CI ランナーの可用性
