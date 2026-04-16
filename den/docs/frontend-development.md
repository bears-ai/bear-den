# Frontend Development Guide

## Overview

This project follows a minimalist frontend approach using server-side rendering, vanilla JavaScript, and custom CSS. This guide provides essential guidelines for maintaining consistency and code quality.

**Templates in production:** non-`production` builds load MiniJinja from the filesystem (`TEMPLATES_DIR`). Release / `--features production` **embeds** templates at compile time—editing files on disk is not enough; rebuild the binary (see [`quickstart.md`](quickstart.md)).

## Key Principles

### 🚫 What NOT to do
- **Never use inline CSS styles** in HTML templates (`style="…"` on elements).
- **Never put authored layout or theme CSS inside `<style>` blocks in templates.** Duplicating `:root` colours, spacing, or component rules in a page file drifts from the design system, bypasses review patterns, and repeats work (for example bear chat historically inlined a full theme until it linked [`style.css`](../src/web/assets/css/style.css) and moved rules to [`specifics.css`](../src/web/assets/css/specifics.css)). **Standalone pages** (full HTML documents that do not extend `base.html`) are not an exception: they still load the same stylesheet entrypoint and keep page-specific selectors in `specifics.css` (or a dedicated imported file), scoped with a class on `<html>` or `<body>` when needed.
- **Don't add rounded corners or gradients** - maintain geometric design
- **Avoid complex JavaScript frameworks** - keep it vanilla
- **Don't put complex logic in templates** - process data in Rust handlers

### ✅ What TO do
- **Use existing CSS classes first** before creating new ones
- **Use CSS variables** for colors, spacing, and typography
- **Process data in handlers** before passing to templates
- **Keep JavaScript simple** and progressive

## CSS Development

### File Structure
- **Main stylesheet**: `src/web/assets/css/style.css` (imports all others)
- **Base styles**: `reset.css`, `common.css`, `layout.css`
- **Feature styles**: `specifics.css` for component-specific styles

### Adding New Styles

#### For a single new class:
Add to the **bottom** of `src/web/assets/css/specifics.css`:

```css
/* -------------------
   Description of feature/component
*/
.new-class-name {
    background-color: var(--page-color);
    border: solid var(--thin-line-unit) var(--border-color);
    padding: var(--spacing-unit);
}
```

#### For multiple related classes:
1. Create new CSS file: `src/web/assets/css/feature-name.css`
2. Import in `src/web/assets/css/style.css`:
   ```css
   @import url("feature-name.css");
   ```

### CSS Variables Reference

#### Colors
- `--page-color` - Main content background
- `--surface-color` - Muted panels and alternate surfaces (for example AI chat bubbles)
- `--field-fill-color` - Background for text inputs, textareas, selects, and the bear chat composer (`#ffffff` in current themes)
- `--text-color` - Primary text
- `--border-color` - Standard borders
- `--accent-color` - Highlights/hovers
- `--selected-color` - Selected states
- `--meta-color` - Secondary text
- `--error-color`, `--warning-color`, `--success-color` - Status colors

#### Spacing
- `--spacing-unit` - Base spacing for padding/margins
- `--thin-line-unit` - Standard border width

#### Typography
- `--body-font-family` - Main body text
- `--data-font-family` - Monospace for data
- `--logo-font-family` - Headings and navigation

## Template Development

### MiniJinja Templates
- **Location**: `src/web/templates/`
- **Process data in Rust handlers** - templates should be simple
- **Use semantic HTML** for accessibility
- **See**: [`minijinja-template-limitations.md`](minijinja-template-limitations.md) for template restrictions

### Standalone HTML documents (no `base.html`)

Some routes render a **full HTML document** without extending `base.html` (for example [`bear_chat.html`](../src/web/templates/bear_chat.html)). Treat them like every other UI surface:

1. **Link the shared stack** — `<link rel="stylesheet" href="/assets/css/style.css" />` (same path as `base.html`). Do not redefine global tokens in the template.
2. **Put layout and chrome in CSS files** — page shell rules live in [`specifics.css`](../src/web/assets/css/specifics.css) (or a file imported from `style.css`), with a **page-scoping class** on `<html>` or `<body>` (for example `html.den-bear-chat-page`) so rules do not leak to the rest of the app.
3. **Add or extend design tokens in `style.css`** — new colours, spacing scales, or JS-readable `--chat-*` (etc.) variables belong next to the rest of the design system, not in a template `<style>` block.
4. **JavaScript may configure third-party components** (for example Deep Chat’s style API) but should **read values via `getComputedStyle` / `var(--…)`** from those tokens, not hard-coded hex or pixel literals that duplicate the system.

If a third-party snippet truly requires a tiny inline or embedded style and cannot consume your classes, document the exception in the PR and keep it minimal.

### Template Structure
```
src/web/templates/
├── base.html              # Base layout
├── [feature]/
│   ├── view.html         # Main template
│   └── edit.html         # Form template
└── admin/                # Admin interface
```

## Bear chat (Deep Chat)

End-user chat is a **standalone MiniJinja document** (it does not extend `base.html`) but it **uses the same [`style.css`](../src/web/assets/css/style.css) import chain** as the rest of the web UI. The page adds a vendored [**Deep Chat**](https://deepchat.dev) web component (`<deep-chat>`).

| Route | Source |
|-------|--------|
| `GET /bear/{slug}` | [`src/web/templates/bear_chat.html`](../src/web/templates/bear_chat.html) (handler in [`bear_chat.rs`](../src/web/bear_chat.rs)) |

**Behavior to preserve**

1. **Same-origin asset** — `deepChat.bundle.js` lives under [`src/web/assets/deep-chat/`](../src/web/assets/deep-chat/) and is linked as `/assets/deep-chat/deepChat.bundle.js` so the shell works without a third-party CDN.
2. **Light / dark theming** — chat loads the same [`style.css`](../src/web/assets/css/style.css) stack as the rest of the web UI (including [`specifics.css`](../src/web/assets/css/specifics.css) for the `.den-bear-chat-page` shell). Tokens such as `--page-color`, `--surface-color`, and the `--chat-*` bridge variables honour `theme-dark` / `theme-light` and `prefers-color-scheme`. Deep Chat's JS style API (`messageStyles`, `textInput`, `submitButtonStyles`, etc.) reads resolved values at init via `getComputedStyle`, and `auxiliaryStyle` uses `var(--…)` so shadow-DOM rules track tokens.
3. **Slack-style layout** — messages are left-aligned (both user and AI), with name labels above and a bottom border divider instead of speech bubbles.
4. **Letta stream** — `POST /v1/chat/send` returns SSE; the chat handler uses `connect.handler` + `connect.stream` to parse `data:` JSON lines. It surfaces **`error_message`** (including nested `contents`) as Deep Chat errors; streams **`reasoning_message`** into a dedicated **HTML** ai bubble (single-line horizontal scroll, optional Expand for full text — plain text only, no Markdown in that bubble per Deep Chat’s `html` vs `text` tradeoff, see [deep-chat#429](https://github.com/OvidijusParsiunas/deep-chat/issues/429)); then **`assistant_message`** as **`text`** (Markdown-friendly). **`overwrite: true`** follows [Deep Chat’s connect `Response` docs](https://deepchat.dev/docs/connect#Stream): overwrite the last ai bubble while reasoning streams, replace reasoning with the first assistant chunk when `hadReasoning`, and concatenate token-streamed assistant chunks that share an `id` (or lack one). See `lettaSseHandler` in `bear_chat.html`.

**Local dev:** After changing anything under `src/web/assets/`, rebuild and restart `den` so `memory-serve` picks up routes and paths ([`quickstart.md`](quickstart.md) § *Static assets*).

## JavaScript Guidelines

### Approach
- **Vanilla JavaScript** - no complex frameworks
- **Progressive enhancement** - pages work without JS
- **Minimal dependencies** — add small scripts only when needed
- **Location**: `src/web/assets/js/` (create files as you add interactivity)

### File Organization
- `[feature].js` - Feature-specific code
- Keep functions simple and focused

## Development Workflow

### Adding a New Feature
1. ✅ Create templates in `src/web/templates/[feature]/` (or a standalone `.html` if the route needs a full document)
2. ✅ Process data in Rust handlers (not templates)
3. ✅ Check existing CSS classes first
4. ✅ Add styles using CSS variables in `src/web/assets/css/` — not in template `<style>` blocks
5. ✅ Add minimal JavaScript if needed
6. ✅ Test without JavaScript

### Code Review Checklist
- [ ] No inline CSS styles on elements
- [ ] No authored `<style>` blocks in templates (standalone pages use `style.css` + `specifics.css`; see [Standalone HTML documents](#standalone-html-documents-no-basehtml))
- [ ] Uses CSS variables
- [ ] No rounded corners/gradients
- [ ] Logic in handlers, not templates
- [ ] Minimal JavaScript
- [ ] Descriptive CSS comments
- [ ] Semantic HTML

## Design System

### Visual Design
- **Geometric shapes** - clean, angular design
- **Solid colors** - no gradients
- **Consistent spacing** - use `var(--spacing-unit)`
- **Monospace data** - use `--data-font-family` for technical info

### Layout Patterns
- **Grid layouts** for complex interfaces
- **Flexbox** for simple alignments
- **Responsive design** with CSS variables for breakpoints

## Performance

### CSS
- **Reuse existing classes** to minimize CSS size
- **Use efficient selectors** - avoid deep nesting
- **Import only needed files**

### JavaScript
- **Lazy load** when possible
- **Minimize DOM manipulation**
- **Use event delegation**

### Templates
- **Process once in handlers** - don't repeat calculations
- **Use caching** for expensive operations

## Common Patterns

### Form Layouts
Use the `.fields-grid` class system from `common.css`

### Admin Interface
Follow existing admin template patterns in `src/web/templates/admin/`

---

## Quick Reference

**CSS Variables**: Check `src/web/assets/css/style.css` for complete list
**Existing Classes**: Browse `src/web/assets/css/` files before creating new ones
**Template Examples**: Look at `src/web/templates/` for patterns
**MiniJinja limits**: [`minijinja-template-limitations.md`](minijinja-template-limitations.md)

**Remember**: When in doubt, keep it simple and follow existing patterns!
