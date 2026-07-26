# Application icons

The web mark at `static/public/psf-guard.svg` is the source for every packaged
desktop application icon. Regenerate the five files named in
`tauri.conf.json` from the repository root:

```bash
./scripts/generate-icons.sh
```

The script uses Tauri's generator in a temporary directory and copies the PNG,
Windows ICO, and macOS ICNS results here. Do not edit generated files by hand.
