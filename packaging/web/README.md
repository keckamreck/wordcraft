# Hosting WordCraft for the web

`wordcraft-web-<version>.zip` (from the GitHub release, or `packaging/web/package.sh`) holds a
static site in `wordcraft-web-<version>/`:

| File | What it is |
|---|---|
| `index.html` | The page. It loads everything through relative URLs. |
| `wordcraft-web-<hash>.js` | wasm-bindgen glue (generated, ES module) |
| `wordcraft-web-<hash>_bg.wasm` | The app, about 13 MB, or 5 MB with compression |
| `manifest.webmanifest`, `icons/` | Web app manifest and icons: the site installs as an app ([below](#install-as-an-app-android-tablets)) |
| `_headers`, `.htaccess` | Sample header rules for Netlify/Cloudflare Pages and Apache |

There is no server-side code. Upload the folder's contents anywhere that serves static files.

## Deploy on Vercel

The repository deploys to [Vercel](https://vercel.com) as it is: `apps/wordcraft-web/vercel.json` tells
Vercel to run `packaging/web/vercel-build.sh` and serve the site, with the headers below.

1. On vercel.com: **Add New… › Project** and import this GitHub repository. Vercel finds several
   Rust programs in it; pick **`apps/wordcraft-web` › Import single project** and keep every setting
   it proposes (they come from that folder's `vercel.json`: no framework preset, no install step).
   Imported with the repository root as the Root Directory instead, the root `vercel.json` does
   the same.
2. **Deploy.** Every later push deploys again by itself: pushes to the default branch go to the
   production URL, pushes to other branches get preview URLs.

The build installs Rust (`RUST_TOOLCHAIN`, pinned in the script) and Trunk (pinned, SHA-256
checked) on Vercel's build machine, and Trunk fetches wasm-bindgen and wasm-opt. A build from
scratch takes about 15–25 minutes on Vercel's standard machine, within its 45-minute limit.
With the Vercel CLI instead: `vercel` (preview) or `vercel --prod` from the repository root.

## Install as an app (Android tablets)

The site is an installable web app (`manifest.webmanifest`): it opens in its own window with
its own icon, without the browser's address bar, in portrait and landscape.

- **Android tablet or phone (Chrome):** open the site, then **⋮ › Add to home screen › Install**
  (or **Install app**). WordCraft then starts from the launcher like any other app. Long-press
  the icon for **Sample document**.
- **ChromeOS, Windows, macOS, Linux (Chrome or Edge):** the install icon at the right of the
  address bar, or **⋮ › Cast, save and share › Install page as app**.
- **iPad (Safari):** **Share › Add to Home Screen**.

Installing needs HTTPS (Vercel serves it). There is no offline mode: the app loads from the
site each time it starts (the large `.wasm` comes from the browser's cache after the first
visit).

On a touch screen the page follows the fingers (`crates/ui-egui/src/touch.rs`):

| Gesture on the page | Does |
|---|---|
| Swipe | Scrolls; a flick coasts on. In Read Mode a swipe turns the page |
| Tap | Puts the caret there, and the on-screen keyboard comes up for typing |
| Double tap / triple tap | Selects the word / the paragraph |
| Touch and hold, then drag | Selects text from where the finger went down (or moves a picture or shape) |
| Touch and hold, then lift | Opens the context menu |
| Two fingers | Pinch to zoom; move together to scroll |

Touches on the ribbon, panes and dialogs act as clicks and drags, and with a Draw tab pen,
pencil, highlighter, eraser or lasso chosen the finger draws on the page. The on-screen keyboard
shrinks the app rather than covering the page (`interactive-widget=resizes-content`). A larger
interface for fingers: **File › Options › General › Interface size**, for example 125%.

## Any path works

All URLs in `index.html` are relative (`public_url = "./"` in `apps/wordcraft-web/Trunk.toml`),
so the site works at a domain root (`https://example.com/`), under a prefix
(`https://example.com/tools/wordcraft/`) and from a CDN bucket. The asset names carry a content
hash, so they can be cached forever. Only `index.html` needs revalidation.

## Required server settings

- **MIME type:** serve `.wasm` as `application/wasm`. Browsers refuse to stream-compile it under
  any other type, and the app then loads slowly or not at all. Serve `.js` as `text/javascript`.
  Most hosts already do both. For nginx, check that `mime.types` has `application/wasm wasm;`.
- **Compression:** turn on gzip or Brotli for `.wasm`, `.js` and `.html`. That takes the
  download from about 13 MB to about 5 MB. You can also precompress (`brotli -k *.wasm`) and let
  the server send `Content-Encoding: br`.
- **Caching:** `Cache-Control: public, max-age=31536000, immutable` on the hashed `.wasm` and
  `.js` files, and `no-cache` on `index.html`.
- **HTTPS:** WebGPU (and the clipboard) only work in a secure context, which means `https://`
  or `http://localhost`. Over plain HTTP elsewhere, the app falls back to WebGL2.
- **No special isolation headers:** WordCraft doesn't use `SharedArrayBuffer`, so it doesn't
  need `Cross-Origin-Opener-Policy` or `Cross-Origin-Embedder-Policy`. If your site already sends
  COEP `require-corp`, also send `Cross-Origin-Resource-Policy: same-origin` (or `cross-origin`
  when the files live on a CDN) on the app's files.

nginx example:

```nginx
location /wordcraft/ {
    types { application/wasm wasm; text/javascript js; text/html html; }
    gzip on;
    gzip_types application/wasm text/javascript text/html;
    location ~* \.(wasm|js)$ { add_header Cache-Control "public, max-age=31536000, immutable"; }
    location ~* index\.html$ { add_header Cache-Control "no-cache"; }
}
```

Local test: `python3 -m http.server 8765` inside the folder, then open http://localhost:8765/.

## Embedding in a page (iframe)

```html
<iframe
  src="https://example.com/wordcraft/"
  title="WordCraft image editor"
  style="width: 100%; height: 720px; border: 0;"
  allow="fullscreen; clipboard-read; clipboard-write"
  allowfullscreen>
</iframe>
```

- The app fills the iframe and follows its size, so size the iframe and not the app.
- Keyboard shortcuts go to the iframe after the user clicks into it, as with any embedded app.
- **Cross-origin embeds** work. Preferences are kept in the iframe's `localStorage`. Browsers
  that partition or block third-party storage may forget them between visits, and the app
  then starts with defaults.
- **Sandboxed iframes** need at least
  `sandbox="allow-scripts allow-same-origin allow-downloads allow-popups"`. Without
  `allow-same-origin` there's no storage. Without `allow-downloads`, Save and Export (browser
  downloads) are blocked.
- Don't send `X-Frame-Options: DENY` or a `frame-ancestors` CSP that excludes the embedding page.

## Renderer selection and fallback flags

WordCraft renders with wgpu. It uses **WebGPU** when the browser has it and falls back to
**WebGL2** on its own. URL query flags override this, and they work on the iframe `src` too:

| Flag | Effect |
|---|---|
| *(none)* | WebGPU if available, otherwise WebGL2 |
| `?webgl` | Force the WebGL2 backend (useful when a WebGPU driver misbehaves) |

For example: `<iframe src="https://example.com/wordcraft/?webgl" ...>`.

A browser with neither WebGPU nor WebGL2 gets a message in place of the app.
