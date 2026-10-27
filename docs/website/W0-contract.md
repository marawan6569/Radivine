# W0 — Content and visual contract

Approved scope: W0 and W1 only, 2026-10-27. Evidence baseline: `fec30dc` and the complete [website plan](../website-plan.md). Product snapshot: v0.3.0. This contract resolves the plan's W0 choices; publication remains unauthorized.

## Visual direction

A precise, editorial desktop-tool identity: generous space, large tightly set Inter headings, quiet green surfaces, hairline dividers and restrained radial guide lines. Preserve the original vine symbol. The HTML “Radivine” name is site typography, not a replacement official wordmark. No stock illustrations, fake application windows, fabricated screenshots, performance counters, or testimonials.

Light is pale mint canvas / white surfaces / deep green ink; Dark is forest canvas / raised green-black surfaces / pale mint ink. System follows the browser independently of the native application. Semantic token contract:

| Role | Light | Dark |
|---|---|---|
| canvas | `#f4fbf8` | `#0a1a16` |
| surface | `#ffffff` | `#10251f` |
| surface-muted | `#e8f3ed` | `#17342b` |
| text | `#102f26` | `#edf7f1` |
| text-muted | `#48665a` | `#a6bdb0` |
| border | `#c8dcd1` | `#355449` |
| accent / link | `#006b50` | `#65dbaf` |
| action background | `#044c3c` | `#65dbaf` |
| action foreground | `#ffffff` | `#09291e` |
| focus | `#00745a` | `#88edc4` |

Typography: local Inter variable, system fallback; body 16–18px, 1.65 line-height, prose maximum 68ch; headings fluid with a 46–100px hero. System monospace for technical labels. Spacing follows a 4px foundation (8/12/16/24/32/48/64/96); maximum page width 1240px. Buttons have at least 44px targets. Transitions use opacity/color/short translation, 140–220ms, no continuous animations, removed under reduced motion. Visible keyboard focus is mandatory. Colors must pass automated contrast checks before W1 completion.

## Homepage composition and information architecture

1. Header: symbol and name, Product, Compatibility, Guide, GitHub; labeled System/Light/Dark selector. Mobile uses a native disclosure and retains usable navigation without JavaScript.
2. Hero: release label; “Branch into action.”; “A context-aware radial action wheel for Linux files.”; concise task-oriented copy; **Get Radivine** and **Explore the product** CTAs. Adjacent release/platform qualifiers stay visible. Desktop uses asymmetric copy/artwork columns; mobile stacks copy, CTAs, qualifiers, artwork. The W1 artwork is expressly brand artwork in a current-capture placeholder, not an application rendering.
3. Product introduction: three compact, connected gesture steps. Branching default and optional Concentric Rings are stated. W1 has a short static introduction only; detailed stories and visual walkthroughs belong to W2.
4. Reserved practice section: plain explanation that the interactive browser demo is not available yet; no pretend controls or dead “Try” CTA. W3 owns the demo.
5. Compatibility checkpoint: primary tested setup; other X11 desktop verification caveat; native Wayland unsupported, before the final release link.
6. Compact reserved feature/guide area: links to existing repository guidance while future W2 native website pages are absent. No empty routes or broken navigation.
7. Final source/contribution invitation and footer: repository, releases, source contribution guidance, project licenses and local asset notices.

The final planned navigation is Try it / Download / Compatibility / Guide / Roadmap / GitHub (plan §5.1). W1 uses real homepage fragments and versioned upstream destinations; Try it is withheld until W3 and Roadmap until W2. Planned W2 routes: download, compatibility, guide (configuration/troubleshooting), roadmap, contribute, licenses. W1 implements home and a helpful 404 only. All internal paths/assets/canonicals derive from the same configurable origin and base; default candidate URL is `https://marawan6569.github.io/Radivine/`, not an existing deployment claim.

## Messaging and evidence boundaries

- Descriptor and tagline above are primary. “Preview text, convert images, and run your own configured commands” accurately describes actions, without implying a general application launcher. Evidence: plan E05–E12; `crates/radivine-actions/src/lib.rs`, conversion and viewer code.
- Download CTA opens the **v0.3.0 GitHub release page**, rather than an unchecked direct package or an unimplemented download route. Label `.deb · amd64 · X11`; use “Get Radivine” / “View v0.3.0 release”. Full installation/checksums/manifest are W2.
- Primary tested setup: **Linux Mint 22.3 · Cinnamon X11 · Nemo · amd64**. Other X11 desktops/file managers need environment-specific verification. Ubuntu container installation does not certify its desktop. Evidence: plan E03/E16/E17 and N5/N6/C5 reports.
- **Native Wayland wheel support is unavailable.** Global Drag + Shift is unavailable on Wayland; XWayland Invoke is experimental/conditional with possible pointer degradation, not a supported native alternative. Evidence: `connect_with_config` selects X11; plan E03.
- Branching is the default; Concentric is optional and may fall back for missing compositing or insufficient viewport/target size. Default concentric footprint is 668×742 logical pixels, dependent on configuration. Evidence: plan E04–E06.
- Default “Open in Terminal”, “Compute SHA-256”, and “Make Executable” examples only log. Never list them as executed built-ins. Executable custom actions are a separately configured argv mechanism requiring installed programs. Evidence: `register_builtins_with_options` registers `LogAction`; plan E07.
- Current: text Full/Head/Tail (read-only), X11 clipboard Copy, conditional Properties, PNG/JPEG/WebP static image conversion, native Advanced Convert/Jobs/Control Center, custom argv actions, both wheel layouts. Limits remain in plan §4; W1 does not expand them into an exhaustive feature page.
- Future/exploring only: Wayland research, additional contexts, dynamic Open With, live-follow preview, third-party action protocols, Windows/macOS, tray and vips adapter. No dates, next-version commitments, or enabled unsupported downloads. Evidence: plan E19. Shipped GUI/conversion/concentric must never appear as upcoming.

## Asset approval and provenance

Originals under `assets/branding/` and `assets/icons/` are immutable. The banner is also preserved. Record original SHA-256 in `website/src/data/assets.json`; build-time derivatives are confined to website output. The build must not require symlinks or visitor requests to external asset hosts.

| Source | Approval / provenance |
|---|---|
| `assets/branding/logo-icon.png` | Inspected original transparent 1254×1254 vine symbol; approved for header and explicit brand artwork; MIT OR Apache-2.0 per THIRD_PARTY_NOTICES. Astro may optimize a website-only derivative. No filtering/inverting. |
| `assets/branding/logo-horizontal.png`, `logo-primary.png` | Opaque white, large margins; keep original; not selected for this shell. |
| `assets/fonts/Inter-Variable.ttf` | Repository Inter by Rasmus Andersson, OFL-1.1; website-only WOFF2 Latin subset permitted; retain original and OFL notice. System fallback if conversion unavailable. |
| `assets/wheel-icons/` | Existing Lucide subset, ISC/MIT notice; approved if needed, preserve notice. W1 can use simple CSS arrows and text without redistributing these icons. |
| `docs/screenshots/wheel-*.png` | Older reference scenes with development paths/extra UI. Not selected as current hero. Any future use requires an explicit historical caption and separately reviewed derivative. |
| `docs/screenshots/control-*.png` | Older exports, dashboard visibly v0.1.1. Not selected or relabeled. |
| `docs/screenshots/viewer-*.png` | Existing references with unconfirmed release provenance; review before publishing. |
| N3 concentric pack | Renderer/sandbox fixtures with debug/sample data; never a live v0.3.0 desktop capture. |

Fresh captures are not a W1 blocker. Current hero placeholder visibly states “Current desktop capture pending” and “Brand artwork · not application UI.” No generated UI or invented photo is authorized.

## Fresh v0.3.0 capture requirements (W2 input)

Use the actual stable app on Mint 22.3 / Cinnamon / X11 / Nemo with neutral disposable files. Capture Branching text-preview path, optional Concentric image-convert path, Advanced Convert, Jobs, and Control Center layout settings; include light/dark examples where useful. Set up the desktop before capture to exclude personal paths, terminal history, unrelated apps, secrets and private filenames. Never edit old version text.

Each selected image needs source file/hash/dimensions, app version and revision, date, OS/desktop/session, display scale, selected configuration/layout, sample fixture description, capture method, kind (live desktop / headless export / sandbox), accurate alt/caption, license and derivative history. Verify actions/menus match the context, inspect privacy, and review the final optimized crop. Headless exports can represent UI but cannot certify desktop integration. Capture approval and public media selection remain W2 review work.

## W0 verification

Read full plan, repository instructions, relevant implementation symbols, original symbol and historical wheel reference. Initial status clean, previous work committed. Content decisions are tied to the plan evidence registry. No assets, Rust, workflows or deployment settings changed. W1 follows directly under the user's explicit W0/W1 authorization; stop after W1.
