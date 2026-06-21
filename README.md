# Taglet

Taglet は、DICOM ファイルのタグを確認・編集するためのデスクトップアプリです。

CT、MR、RT Plan、RT Structure Set、RT Dose などの DICOM ファイルを開き、タグの内容をツリー形式で確認できます。一部の値はその場で編集して保存できます。

> Taglet は開発中のソフトウェアです。臨床判断や診療行為に直接使用しないでください。

## 主な機能

- DICOM タグのツリー表示
- Sequence / Item の展開・折りたたみ
- タグ番号またはタグ名による検索
- テキスト系 VR の編集
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
cd src-tauri
cargo check
cargo test
```

## ライセンス

MIT
