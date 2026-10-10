# roomy

[English](README.md) | **日本語**

**Mac をいつも広々と保つ、きれい好きなルームメイト。**

roomy は Mac のストレージとメモリを見張り、足りなくなる**前に**知らせ、何を消せば安全かを
判断の根拠とあわせて示します。開発者の Mac が実際に溜め込む場所 — git worktree、Xcode の
シミュレータ、パッケージのキャッシュ、ツールの旧バージョン、エディタの拡張機能、Docker、
AI モデル — を理解しています。

**人と同じくらい AI エージェントのために作られています。** Claude Code や Codex などのエージェントは、
`du` や `rm` を即興で使う代わりに roomy（CLI または MCP）を呼びます。小さく決まった形のレポートを受け取り、
あなたに説明し、承認されたものだけを同じ安全装置を通して削除します。

## roomy の特徴

- **開発者特有の溜まり方がわかる。** worktree は、未コミットの変更がなく、コミットがリモートにあり、
  誰も作業しておらず、しばらく使われていないときだけ候補にします。消えてしまう Git 管理外の
  ファイル（`.env` など）があれば一覧にし、「要確認」にします。
- **理由を示す。** 「push 済み · 未コミットの変更なし · 最終操作 12 日前」「最新 2 件は残す」
  「エディタが廃止済みとマーク」など、すべての候補に根拠が付きます。
- **増えた場所を見つける。** 並列の 1 回の走査で大きなフォルダの容量を記録し、次のスキャンで
  どこが増えたかを、親ではなく原因に近いフォルダで示します。
- **実際に空く量を数える。** APFS クローン（シミュレータのデバイス、`cp -c` の複製）はブロックを
  共有するため、33 GB あるフォルダを消しても 10 GB しか空かないことがあります。削除候補については
  実際に空く量を測ってサイズと並べて示し、合計にもこちらを使います。
- **2 種類の空きを示す。** 「空き」は `df` と同じ未使用の領域で、警告はこれで判定します。
  「パージ可能込み」は macOS が必要に応じて消す領域（キャッシュ、ローカルスナップショット）を
  足した値で、Finder やシステム設定が「利用可能」と表示するものです。
- **早めに知らせる。** 小さな常駐エージェントが 15 分ごとに確認し、空きが少ない・急に減った・
  メモリ逼迫やスワップが多いときに通知します。

## 安全性の約束

- **自動では消しません。** 削除は `roomy clean`（ダッシュボードでは `c`）で、内容を示して確認してからです。
- **削除の直前に必ず再確認します。** スキャン後に worktree へコミットが入った、アプリが起動した、
  フォルダが消えたなどの変化があれば、その候補は飛ばします。
- **ガードが拒否します**: ホームの外、ホームに近すぎる場所、保護フォルダ（`~/Documents`、`~/Desktop`、
  `~/.ssh`、キーチェーン、メール、iCloud など）の中や上位、シンボリックリンク経由のパス。
- **ツール自身の掃除コマンドを優先します**（`npm cache clean`、`uv cache clean`、`brew cleanup`、
  `git worktree remove`、`xcrun simctl delete unavailable` など）。
- **再生成できるものは削除、ユーザーデータはゴミ箱へ。** アプリは消さず、アンインストーラを案内します。
- **削除はすべて記録します**（`~/Library/Application Support/roomy/audit.jsonl`）。
- **テレメトリはありません。** 何も外部に送りません。

## インストール

```sh
brew install masa-tichita/tap/roomy
brew services start roomy          # 常駐チェックと日次スキャン
```

Apple Silicon と Intel の両方で動く Universal バイナリです（macOS 13 以降）。Rust 1.88 以降で自分でビルドする場合:

```sh
cargo install --git https://github.com/masa-tichita/roomy
roomy service install              # Homebrew を使わない場合の常駐チェック
```

シェル補完と man ページは Homebrew なら自動で入ります。`cargo install` の場合:

```sh
roomy completions zsh > ~/.zfunc/_roomy     # bash, fish, elvish, powershell も可
roomy man --dir ~/.local/share/man/man1     # roomy.1 とコマンドごとのページ
```

## 使い方

```sh
roomy                      # ダッシュボード（ターミナルで実行したとき）
roomy status               # 空き容量・メモリ・スワップ・メモリ上位アプリ
roomy scan                 # ディスクを走査し、候補と増えた場所を表示
roomy clean 1 3 --dry-run  # 何が起きるかを事前に確認
roomy clean 1 3            # 実行（確認あり）
brew services start roomy  # 常駐チェックと日次スキャン（または roomy service install）
roomy doctor               # 権限・ツール・エージェント・設定の診断
```

## フルディスクアクセス

なくても動きます。フルディスクアクセスがない場合、**バックグラウンド**のスキャンは macOS が保護する
フォルダ（デスクトップ・書類・ダウンロード・他アプリのデータ）を走査しないので、予期しない許可
ダイアログは出ません。ターミナルから実行したスキャンはターミナルの権限で動きます。

バックグラウンドでもそれらを対象にするには、*システム設定 › プライバシーとセキュリティ ›
フルディスクアクセス* に `roomy` を追加してください。署名付きバイナリを配布するまでは、macOS が
許可をビルドごとに紐づけるため、アップグレードのたびに追加し直す必要があります。

## 設定

`roomy config init` でコメント付きの `~/.config/roomy/config.toml` を作成します。項目はすべて省略可能です。
例は [README.md](README.md#configuration) を参照してください。`roomy rules` でルール一覧、
`roomy config validate` で打ち間違いを確認できます。

## AI エージェントから使う

エージェントは、何ページもの `du` の出力ではなく数 KB の JSON で同じ結果を受け取り、roomy が
消さないものは消せません。

**MCP**（ツール: `roomy_status`、`roomy_report`、`roomy_scan`、`roomy_explain`、`roomy_top`、
`roomy_history`、`roomy_doctor`、`roomy_plan_clean`、`roomy_clean`）:

```sh
claude mcp add --scope user roomy -- roomy mcp          # Claude Code
```

```toml
# Codex: ~/.codex/config.toml
[mcp_servers.roomy]
command = "roomy"
args = ["mcp"]
```

`roomy mcp --read-only` で削除ツールを除けます。シェルの `PATH` を引き継がないデスクトップアプリでは
バイナリのフルパス（`which roomy`）を指定してください。

**CLI ＋ 説明書**（コマンドを実行できるエージェントなら何でも）:

```sh
roomy guide --install-claude-skill     # Claude Code のスキルを ~/.claude/skills/roomy に
roomy guide --format agents-md         # AGENTS.md / CLAUDE.md に貼る文面
roomy guide                            # エージェントが従う手順
```

手順: `report --json` → `explain <id> --json` → ユーザーに提案 → `clean <id> --json`（ドライラン）
→ 承認後に `clean <id> --json --yes`。

**無人実行の歯止め。** `--yes`・`--json`・MCP では、「要確認」の候補は `--allow-review`（`allow_review`）
が必要で、1 回に消せる量は `[clean] max_unattended_gb`（既定 50 GB）までです。削除は
`"unattended": true` 付きで記録されます。

**出力の約束。** `status`・`scan`・`report`・`explain`・`clean`・`top`・`history`・`rules`・`doctor`
は `--json` に対応。JSON は常に `"schema": 1` を含み、文字列は英語、エラーは
`{"schema": 1, "error": "…"}` です。パイプ先には詰めて、ターミナルには整形して出力します。
`report --json` は要約（パス一覧なし）で、`--full` で全項目を出します。

| 終了コード | 意味 |
|---|---|
| 0 | 成功 |
| 1 | エラー、または `clean` の一部が失敗 |
| 2 | コマンドラインの誤り |
| 3 | `status --exit-code` で空き容量かメモリが閾値を超えている |

## アンインストール

```sh
brew services stop roomy && brew uninstall roomy   # または roomy service uninstall && cargo uninstall roomy
rm -r ~/Library/Application\ Support/roomy        # レポート・スナップショット・履歴・監査ログ
rm -r ~/.config/roomy                              # 任意: 設定
```

## ライセンス

[Apache License 2.0](LICENSE-APACHE) または [MIT License](LICENSE-MIT) のいずれかを選択できます。
