# circle-search

Circle to Search for Windows. See `CLAUDE.md` for non-negotiables, stack, and
the state machine, and `../circle-to-search-plan.md` for the full phased plan.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

## Dev

```
npm install
npm run tauri dev
```

Keep this command running while testing. The development overlay loads from
Vite on `localhost:1420`; stopping the command makes an already-running dev
window show a connection error.

To build a standalone executable that works without Vite, use:

```
npm run tauri build -- --no-bundle
```

Run `src-tauri/target/release/circle-search.exe` only after that command
finishes. Plain `cargo build --release` does not package the frontend assets
and can leave the executable pointing at the development server.

Press Shift+Enter to freeze the screen, then draw with the lasso or rounded
rectangle tool. The selection panel offers **Select Text**, **Translate**,
**Google Lens**, **Image to ChatGPT**, and **Save PNG**. Nothing is sent
anywhere until you choose an external action.

**Select Text** runs OCR only for that lassoed area. Drag across the detected
text to reveal **Copy**, **Translate**, and **Ask AI**. **Image to ChatGPT**
opens ChatGPT with the selected image on your clipboard; paste it into the
composer with Ctrl+V. Press Esc to dismiss, or right-click the Circle Search
tray icon and choose **Exit Circle Search** to quit the app.

## Third-party models

- **U²-Net-P** (Snap, downloaded on first use): Apache-2.0, © Xuebin Qin et al. — https://github.com/xuebinqin/U-2-Net
