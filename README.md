# Taglet

Taglet is a desktop DICOM tag editor built with Rust, Tauri v2, React, and TypeScript.

It is currently an early-stage application focused on practical DICOM tag inspection and editing, with a basic image viewer foundation for uncompressed grayscale images.

## Features

- Open DICOM files and inspect tags in a tree table
- Expand and collapse sequences
- Inline edit supported text-like VRs
- Multi-value VR validation, for example `DS = 0\0\0`
- Add and delete tags
- Add tags to the root dataset or a selected sequence item
- Save and Save As
- Lightweight initial open for image files by avoiding eager Pixel Data loading
- Basic image viewer metadata panel
- Basic uncompressed grayscale image display
- WL/WW controls
- Zoom, pan, and fit
- Multi-frame frame slider

## Current Image Support

Prepared rendering path:

- Transfer Syntax:
  - Implicit VR Little Endian
  - Explicit VR Little Endian
- Photometric Interpretation:
  - `MONOCHROME1`
  - `MONOCHROME2`
- Bits Allocated:
  - 8
  - 16
- Single-frame and multi-frame grayscale images

Compressed transfer syntaxes, RGB images, RT Dose 3D views, and Beam's Eye View are planned but not implemented yet.

## Development

Install dependencies:

```bash
npm install
```

Run frontend build checks:

```bash
npm run build
```

Run Rust checks:

```bash
cd src-tauri
cargo check
cargo test
```

Run the desktop app:

```bash
npm run tauri:dev
```

For WSLg environments where GPU/EGL warnings are noisy, try:

```bash
npm run tauri:dev:wsl
```

## Versioning

Use `package.json` as the version source of truth.

```bash
npm version patch
```

The npm `version` lifecycle runs `scripts/sync-version.mjs`, which syncs the new version to:

- `package-lock.json`
- `src-tauri/Cargo.toml`

If the files ever drift, run:

```bash
npm run version:sync
```

## Updates

Taglet is intended to use GitHub Releases from `wkt84/taglet` for app updates.

For Tauri updater support, generate a signing key pair locally, keep the private key out of git, and store it in GitHub Actions secrets. Only the public key should be embedded in the app configuration.

Recommended GitHub Actions secrets:

- `TAURI_SIGNING_PRIVATE_KEY`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`

Planned updater endpoint:

```text
https://github.com/wkt84/taglet/releases/latest/download/latest.json
```

## Tech Stack

- Rust
- Tauri v2
- dicom-rs
- React
- TypeScript
- TanStack Table
- Tailwind CSS
- Vite

## License

MIT
