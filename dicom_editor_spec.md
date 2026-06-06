# Taglet — Claude Code 指示書

## プロジェクト概要

**Taglet** は Rust + Tauri v2 + React (TypeScript) によるクロスプラットフォーム DICOM タグエディタである。
Windows / macOS でシングルバイナリとして動作することを目標とする。

---

## 開発環境

### 推奨環境：WSL2 (Ubuntu 22.04 / 24.04)

Tauri v2 の開発・ビルドは WSL2 上で行う。

#### 必要パッケージ（Ubuntu）

```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Node.js（nvmを推奨）
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.39.7/install.sh | bash
nvm install --lts

# Tauri v2 が必要とするシステムライブラリ
sudo apt update
sudo apt install -y \
  libwebkit2gtk-4.1-dev \
  librsvg2-dev \
  patchelf \
  build-essential \
  curl \
  wget \
  file \
  libssl-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev
```

#### WSL2 での GUI 表示について

| 環境 | 状況 |
|---|---|
| Windows 11 + WSLg | `tauri dev` でそのままウィンドウが表示される（推奨） |
| Windows 10 WSL2 | WSLg がないため GUI 表示不可。後述のフロントエンド単体確認で代替 |

Windows 10 の場合、フロントエンドの動作確認は以下で行う：

```bash
# Vite dev server をブラウザで確認（Tauri なしでフロントのみ）
npm run dev
# → http://localhost:5173 をブラウザで開く
# Tauri コマンドはモックに差し替えて開発する
```

#### Windows 向けバイナリのビルド（クロスコンパイル）

WSL2 から Windows 向けバイナリを生成する場合：

```bash
# Windows ターゲットを追加
rustup target add x86_64-pc-windows-gnu

# MinGW ツールチェーンを追加
sudo apt install -y mingw-w64

# ビルド
cargo tauri build --target x86_64-pc-windows-gnu
```

> **注意**：Tauri の Windows ビルドは NSIS インストーラを生成するため、
> 完全な `.exe` シングルバイナリを得るには Windows 環境または
> GitHub Actions（後述）でのビルドを推奨する。

---

### 代替環境：GitHub Actions によるクラウドビルド

ローカルで `tauri dev`（動作確認）を行い、リリースビルドは GitHub Actions に任せる構成。

#### `.github/workflows/build.yml`（参考）

```yaml
name: Build Taglet

on:
  push:
    tags:
      - 'v*'

jobs:
  build:
    strategy:
      matrix:
        include:
          - os: windows-latest
            target: x86_64-pc-windows-msvc
          - os: macos-latest
            target: x86_64-apple-darwin
          - os: macos-latest
            target: aarch64-apple-darwin   # Apple Silicon

    runs-on: ${{ matrix.os }}

    steps:
      - uses: actions/checkout@v4

      - name: Setup Node.js
        uses: actions/setup-node@v4
        with:
          node-version: 20

      - name: Setup Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Install dependencies
        run: npm ci

      - name: Build Tauri app
        uses: tauri-apps/tauri-action@v0
        with:
          args: --target ${{ matrix.target }}

      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: taglet-${{ matrix.os }}
          path: src-tauri/target/${{ matrix.target }}/release/bundle/
```

#### GitHub Codespaces（フロントエンド開発のみ）

Tauri の GUI は Codespaces で動かないが、フロントエンドの開発・確認には使える。

```bash
# Tauri コマンドをモック化した状態で Vite dev server を起動
npm run dev
# Codespaces が自動的にポートフォワードしてブラウザで確認できる
```

---

### 推奨ワークフロー（まとめ）

```
日常開発
  └─ WSL2 (Windows 11) または macOS
       ├─ フロントエンド確認: npm run dev（ブラウザ）
       └─ 統合確認: cargo tauri dev（ネイティブウィンドウ）

リリースビルド
  └─ git tag v0.x.x && git push --tags
       └─ GitHub Actions が Windows / macOS バイナリを自動生成
```

---

## 技術スタック

| レイヤー | 採用技術 |
|---|---|
| バックエンド | Rust + [dicom-rs](https://github.com/Enet4/dicom-rs) |
| デスクトップフレームワーク | Tauri v2 |
| フロントエンド | React 18 + TypeScript |
| テーブル | TanStack Table v8 |
| スタイリング | Tailwind CSS v3 |
| ビルドツール | Vite |

---

## ディレクトリ構成

```
taglet/
├── src-tauri/
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── commands/
│       │   ├── mod.rs
│       │   ├── file.rs       # open / save
│       │   └── validate.rs   # VRバリデーション
│       └── dicom/
│           ├── mod.rs
│           ├── parser.rs     # dicom-rs ラッパー
│           ├── model.rs      # DicomNode (タグツリー型)
│           └── vr.rs         # VR別バリデーションロジック
├── src/
│   ├── main.tsx
│   ├── App.tsx
│   ├── components/
│   │   ├── TagTable.tsx      # メインのツリーテーブル
│   │   ├── TagRow.tsx        # 1行分（inline編集対応）
│   │   ├── ValueCell.tsx     # VR種別に応じた入力UI
│   │   └── Toolbar.tsx       # Open / Save ボタン
│   ├── hooks/
│   │   └── useDicomFile.ts   # Tauri commandsとのブリッジ
│   └── types/
│       └── dicom.ts          # 共有型定義
├── .github/
│   └── workflows/
│       └── build.yml         # クロスプラットフォームビルド
└── package.json
```

---

## データモデル（Rust / TypeScript 共通）

### DicomNode（Rust 側定義・JSON でフロントに渡す）

```rust
// src-tauri/src/dicom/model.rs

#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "kind")]
pub enum DicomNode {
    Element {
        tag: String,          // "(0008,0010)" 形式
        vr: String,           // "LO", "DA", "SQ" など
        description: String,  // タグ名（データディクショナリから）
        value: String,        // 表示用文字列
        length: u32,
        path: Vec<String>,    // ネスト位置を示すパス（編集時に使用）
    },
    Sequence {
        tag: String,
        description: String,
        length: u32,
        path: Vec<String>,
        items: Vec<Vec<DicomNode>>,  // Sequence Items のリスト
    },
}
```

### TypeScript 側の型

```typescript
// src/types/dicom.ts

export type DicomElement = {
  kind: 'Element'
  tag: string
  vr: string
  description: string
  value: string
  length: number
  path: string[]
}

export type DicomSequence = {
  kind: 'Sequence'
  tag: string
  description: string
  length: number
  path: string[]
  items: DicomNode[][]
}

export type DicomNode = DicomElement | DicomSequence

export type ValidationResult = {
  valid: boolean
  message?: string
}
```

---

## Tauri Commands（Rust → TypeScript インターフェース）

```rust
// 実装する Tauri commands 一覧

// ファイルを開いてタグツリーを返す
#[tauri::command]
async fn open_dicom_file(path: String) -> Result<Vec<DicomNode>, String>

// 編集済みのタグを保存する
#[tauri::command]
async fn save_dicom_file(path: String, nodes: Vec<DicomNode>) -> Result<(), String>

// 名前を付けて保存（保存先はフロントからダイアログで取得して渡す）
#[tauri::command]
async fn save_dicom_file_as(path: String, nodes: Vec<DicomNode>) -> Result<(), String>

// VRバリデーション（入力中にリアルタイムで呼び出す）
#[tauri::command]
fn validate_value(vr: String, value: String) -> ValidationResult
```

---

## VR バリデーション仕様

以下のVRについてRust側でバリデーションを実装すること。

| VR | 検証内容 |
|---|---|
| `DA` | YYYYMMDD 形式、存在する日付であること |
| `TM` | HHMMSS または HHMMSS.FFFFFF 形式 |
| `UI` | 数字とドット(.)のみ、先頭・末尾ドット禁止、64文字以内 |
| `IS` | 整数（符号付き可）、12文字以内 |
| `DS` | 十進数文字列、16文字以内 |
| `CS` | 大文字英字・数字・スペース・アンダースコアのみ、16文字以内 |
| `LO` | 64文字以内（制御文字禁止） |
| `SH` | 16文字以内（制御文字禁止） |
| `PN` | `^` および `=` を区切り文字とする人名形式 |
| `SQ` | 値の編集不可（子ノードのみ編集） |

---

## フロントエンド実装仕様

### TagTable コンポーネント

- TanStack Table v8 の `getExpandedRowModel` を使ってSequenceの展開/折りたたみを実装
- Sequenceの行は左端に `▶` / `▼` トグルアイコンを表示
- Sequence Item の区切りは背景色で視覚的に分ける
- カラム構成：Tag | Description | VR | Value | Length

### ValueCell コンポーネント（インライン編集）

- 通常時は値をテキスト表示
- セルをクリックすると編集モードになる（`contentEditable` または `<input>`）
- 編集中は Tauri の `validate_value` コマンドを呼んでリアルタイムバリデーション
- バリデーションエラー時は赤いボーダー＋エラーメッセージをツールチップ表示
- `SQ` VR の行は編集不可（クリックしても編集モードにならない）
- `Escape` キーで編集をキャンセル、`Enter` キーで確定

### Toolbar コンポーネント

- `Open` ボタン：Tauri の `dialog.open` でファイル選択 → `open_dicom_file` コマンド呼び出し
- `Save` ボタン：現在開いているパスに上書き保存
- `Save As` ボタン：`dialog.save` でパスを選択して保存
- ファイルを開いていない状態では Save/Save As を disabled にする
- タイトルバーに `Taglet — {ファイル名}` を表示する

---

## スタイル方針

- ベースは Tailwind CSS
- テーブルの配色は DicomEdit（スクリーンショット参照）に近い落ち着いたトーン
  - ヘッダー行：濃いグレー背景
  - 通常行：白／薄グレー交互
  - Sequence行：わずかに青みがかった背景
  - 選択行：青ハイライト
  - エラーセル：赤いボーダー

---

## Cargo.toml 依存関係（参考）

```toml
[dependencies]
tauri = { version = "2", features = [] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
dicom = "0.7"          # dicom-rs umbrella crate
tokio = { version = "1", features = ["full"] }
```

---

## 実装順序（推奨）

1. **Tauri プロジェクト初期化**
   ```bash
   npm create tauri-app@latest taglet -- --template react-ts
   ```

2. **Rust バックエンド**
   - `dicom-rs` でファイルを開き `Vec<DicomNode>` を返す `open_dicom_file` コマンドを実装
   - フラットなタグリストから Sequence をネストした `DicomNode` ツリーを構築するロジック

3. **フロントエンド骨格**
   - `useDicomFile` フックでファイルを開いて `DicomNode[]` を state に持つ
   - TanStack Table でフラット表示（Sequence展開なし）

4. **Sequence展開/折りたたみ**
   - TanStack Table の `subRows` にマッピングして展開対応

5. **インライン編集 + バリデーション**
   - `ValueCell` でクリック編集 + Rust側バリデーション呼び出し

6. **保存機能**
   - 変更済みノードを Rust に渡して `dicom-rs` で書き出し

7. **GitHub Actions 設定**
   - Windows / macOS のクロスプラットフォームビルドを自動化

---

## 注意事項

- Pixel Data タグ（`(7FE0,0010)`）は Value を `[Binary Data]` と表示し編集不可にすること
- Private タグ（奇数グループ）はDescriptionを `[Private]` と表示すること
- Transfer Syntax は読み込み時のものを保持して保存すること（変換しない）
- dicom-rs の `dicom::object::open_file` が返す `DefaultDicomObject` を再帰的にトラバースして `DicomNode` に変換する際、Sequence Item のネストを正確に反映すること
