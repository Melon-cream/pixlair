# Pixlair

Pixlair は、対話 `codex` セッションの横で動く小さな companion TUI です。殺風景なターミナルに可愛さを求めるものであり、生産性はありません。
現状は Codex のみに対応しています。
English version: [README-en.md]()

## 機能

- 対話 `codex` 専用のアバター sidecar
- `zellij` のペイン構成に最適化
- CLI のセッションイベントに応じた表情変化
- 端末表示向けに調整したコンパクトな Unicode アバター
- 見た目確認用の `--demo` モード
- 手動確認用の外部イベントフィード対応

## 必要環境

- Rust toolchain
- Unix 系ターミナル
- `zellij`
- `PATH` 上で実行できる `codex`
- 対話 TTY

## ビルド

```bash
cargo build
```

## 使い方

### Codex と一緒に起動

`zellij` の attach 済みセッションの中で実行します。

```bash
cargo run -- --codex
```

`--codex` の後ろに書いたものは、そのまま `codex` CLI への引数として渡されます。

```bash
cargo run -- --codex resume --last
cargo run -- --codex --no-alt-screen
```

このモードでは次のように動作します。

- Pixlair が左側に side pane を開く
- 現在の pane で `codex` が起動する
- `~/.codex/sessions/...` のセッションファイルを Pixlair が監視する
- thinking、tool、working、success などの状態に応じてアバター表情が切り替わる

### デモモード

```bash
cargo run -- --demo
```

Codex を起動せず、表情を自動で切り替えながら表示します。

### 引数なし

引数なしで起動した場合は help を表示します。

```bash
cargo run
```

## 状態の扱い

Pixlair は Codex のセッション JSONL を見て、主に次の状態へ変換します。

- `thinking`
- `tool`
- `working`
- `success`
- `error`
- `input`

実際の判定には reasoning、tool call、assistant message、task completion などのイベントを使います。

## キー操作

フル TUI で動作している場合のキーです。

- `1` idle
- `2` input
- `3` thinking
- `4` working
- `5` success
- `6` error
- `7` sleeping
- `8` tool
- `d` demo mode の切り替え
- `h` help の表示切り替え
- `q` 終了

## 外部イベントフィード

手動テスト用に、外部イベントファイルの tail にも対応しています。

```bash
touch /tmp/pixlair.events
cargo run -- --events /tmp/pixlair.events
```

例:

```text
state=thinking message="Planning the change"
state=success badge=ok message="Patch applied"
```

```json
{"state":"tool","tool":"wrench","message":"Running formatter","badge":"run"}
```

## 制約

- 通常利用は attach 済みの `zellij` セッション内を前提にしています
- 端末ごとの Unicode 幅実装差で見え方が変わる場合があります
- 画像レンダラではなく軽量な ANSI 制御で描画しています
- 対話 Codex の追従は `~/.codex/sessions` 配下のセッションファイルに依存します

## ライセンス

MIT License。全文は `LICENSE.md` を参照してください。
