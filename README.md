# Taglet

Taglet は、DICOM ファイルのタグを確認・編集するためのデスクトップアプリです。

CT、MR、RT Plan、RT Structure Set、RT Dose などの DICOM ファイルを開き、タグの内容をツリー形式で確認できます。一部の値はその場で編集して保存できます。

> Taglet は開発中のソフトウェアです。臨床判断や診療行為に直接使用しないでください。

## 主な機能

- DICOM タグのツリー表示
- Sequence / Item の展開・折りたたみ
- タグ番号またはタグ名による検索
- テキスト系 VR の編集
- 同じタグ番号の一括変更（親 Sequence 配下・ファイル全体・複数ファイル）
- multiple value の validation
- Tag / Sequence の追加と削除
- Save / Save As
- 複数 DICOM ファイルをタブで表示
- ドラッグアンドドロップでファイルを開く
- Pixel Data タグを含む DICOM の軽量オープン
- CT / MR / RT Image / RT Dose などの画像表示
- WL / WW 調整
- zoom / pan / fit
- multi-frame 表示
- RT Plan の Beam's Eye View 表示
- RT Structure Set のスライス別輪郭表示
- GitHub Releases を使った自動更新確認

## インストール

最新版は GitHub Releases からダウンロードできます。

https://github.com/wkt84/taglet/releases/latest

### Windows

通常は次のファイルをダウンロードしてください。

```text
Taglet_<version>_Windows_x64.msi
```

`setup.exe` がある場合は、そちらも Windows x64 向けのインストーラーです。

```text
Taglet_<version>_Windows_x64-setup.exe
```

### macOS

Intel Mac の場合:

```text
Taglet_<version>_macOS_Intel.dmg
```

Apple Silicon Mac の場合:

```text
Taglet_<version>_macOS_AppleSilicon.dmg
```

### macOS で起動できない場合

現在の Taglet は Apple Developer ID による署名・notarization を行っていません。そのため macOS では、初回起動時に開発元を確認できない旨の警告が出ることがあります。

Taglet の配布元を信頼できる場合は、インストール後に次のコマンドで quarantine 属性を削除すると起動できます。

```bash
xattr -cr /Applications/Taglet.app
```

この操作に不安がある場合は、Release のバイナリを使用せず、ソースコードから自分でビルドしてください。

## 使い方

1. `Open` から DICOM ファイルを選択します。
2. タグ一覧が表示されます。
3. Sequence 行または Item 行をクリックすると、展開・折りたたみできます。
4. 検索欄にタグ番号やタグ名を入力すると、該当タグを探せます。
5. 編集可能な値は表の Value 欄で変更できます。
6. `Save` または `Save As` で保存します。

複数ファイルを開いた場合は、画面上部のタブで切り替えできます。

### タグの一括変更

編集可能なタグ行を選択して `Batch Edit` を押すか、行を右クリックして `Batch edit this tag…` を選びます。

- `Files`: 現在のファイル、または開いている複数ファイルから対象を選択します。
- `Scope`: ファイル全体、または親 Sequence 配下の全 Item を指定します。入れ子の Sequence も対象です。
- `New value`: 対象タグに設定する値を入力します。空欄にすると値を空にできます。
- `Preview`: ファイル、Sequence / Item の位置、Beam 名・番号、現在値、新しい値を確認します。チェックを外すと個別に除外できます。

例えば RTPlan の Beam 内のマシン ID を選び、Scope を Beam Sequence にすると、全 Beam の同じタグ番号をまとめて変更できます。複数ファイルでは Item 番号ではなく Sequence のタグ番号の階層で範囲を照合するため、Beam 数が異なるファイルも対象にできます。

一致するタグがないファイルは一覧に表示され、タグは追加されません。編集不可のタグと、既に新しい値になっているタグは変更対象外です。適用前に各 VR の値検証を行い、検証に失敗した場合は一括変更全体を適用しません。

Private タグ（Private Creator を含む奇数グループ）は一括変更の対象外です。テキスト系 VR の Private タグは個別編集できます。

適用後は未保存の状態です。変更した各ファイルを `Save` または `Save As` で保存してください。

### Private タグ

Private タグの Description は `[Private]` と表示します。読み込み時に Sequence と認識されたものは、通常の Sequence と同じように展開できます。メーカー固有の辞書や、`UN` の内容から Sequence を推定する機能はありません。

Private の `UN` がバイト配列で、末尾の空白・NULLを除いて 1〜10,240 バイトの印字可能 ASCII のみなら、文字列として表示します。改行・タブ・途中の NULL・非 ASCII 文字を含む値はこの文字列表示の対象外です。値の形から `CS?`・`SH?`・`LO?`・`LT?` を参考表示しますが、元の VR は `UN` のままで編集不可です。表示用の文字列で保存データを置き換えません。`LT?` は長さが 64 バイトを超え、複数値の区切りであるバックスラッシュを含まない場合の推定です。

保存前に Private Creator の整合性を確認します。追加・編集する Private 要素には、同じデータセットまたは同じ Sequence Item 内の、対応するグループ・ブロックの有効な `LO` Creator が必要です。対応する要素（Private Sequence を含む）を残したまま、その Creator を変更・空にする・削除する操作は保存できません。Creator を削除する場合は、そのブロックの要素も削除してください。親データセットや別 Item の Creator は使いません。

元から Creator が欠けている要素を変更せずに保持する場合は保存を許可します。検証に失敗した場合、変更はファイルにも保存用オブジェクトにも適用されません。

## Viewer

`Viewers` メニューから、ファイルの種類に応じた表示機能を開けます。

### Image Viewer

Pixel Data を持つ DICOM 画像を表示します。

対応している主な画像:

- CT
- MR
- RT Image
- RT Dose
- uncompressed grayscale image
- multi-frame image

主な操作:

- WL / WW 調整
- 簡易ヒストグラム操作
- zoom / pan / fit
- multi-frame の frame 切り替え

### BEV Viewer

RT Plan の Beam's Eye View を表示します。

主な表示内容:

- Beam 選択
- Control Point 選択
- Jaw
- MLC
- leaf width
- collimator angle

### RT Structure Viewer

RT Structure Set の輪郭をスライスごとに表示します。

主な操作:

- スライス選択
- ROI の表示 / 非表示
- zoom / pan / fit

## 対応状況

Taglet は現在、以下を中心に対応しています。

- Implicit VR Little Endian
- Explicit VR Little Endian
- grayscale image
- 8 / 16 / 32 bit pixel data
- single-frame / multi-frame
- RT Plan BEV
- RT Structure Set contour

圧縮画像、RGB 画像、高度な 3D 表示、CT との RTSTRUCT overlay などは今後の検討対象です。

## 注意事項

- Taglet は DICOM ファイルを編集できるため、元ファイルのバックアップを取ってから使用してください。
- 編集後のファイルが、すべての DICOM システムで受け入れられることは保証されません。
- Private tag や装置固有のタグは、意味を十分に確認してから編集してください。
- 医療機器としての認証を受けたソフトウェアではありません。

## 開発者向け

依存関係のインストール:

```bash
npm install
```

開発起動:

```bash
npm run tauri:dev
```

WSLg で描画まわりの警告が出る場合:

```bash
npm run tauri:dev:wsl
```

チェック:

```bash
npm run build
npm test
cd src-tauri
cargo check
cargo test
```

`npm test` は Node.js 24 で検証しています。

### 画面テスト（Playwright）

初回はテスト用 Chromium をインストールします。

```bash
npx playwright install chromium
npm run test:e2e
```

Playwright が Vite を `http://127.0.0.1:5174` で起動し、Chromium で一括変更画面をテストします。Tauri API はテスト側からモックを注入し、2 Beam / 3 Beam の RTPlan 相当データを読み込みます。Sequence の範囲、複数ファイル、個別除外、検証エラー、空値への変更と、保存コマンドに渡される編集結果を確認します。実ファイルの読み書きと Rust 側の値検証は、この画面テストの対象外です。

画面を表示して実行する場合:

```bash
npm run test:e2e -- --headed
```

失敗時のスクリーンショットと trace は `test-results/` に保存されます。

## ライセンス

MIT
