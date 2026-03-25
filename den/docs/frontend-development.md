# Frontend Development Guide

## Overview

This project follows a minimalist frontend approach using server-side rendering, vanilla JavaScript, and custom CSS. This guide provides essential guidelines for maintaining consistency and code quality.

**Templates in production:** non-`production` builds load MiniJinja from the filesystem (`TEMPLATES_DIR`). Release / `--features production` **embeds** templates at compile time—editing files on disk is not enough; rebuild the binary (see [`quickstart.md`](quickstart.md)).

## Key Principles

### 🚫 What NOT to do
- **Never use inline CSS styles** in HTML templates
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

### Template Structure
```
src/web/templates/
├── base.html              # Base layout
├── [feature]/
│   ├── view.html         # Main template
│   └── edit.html         # Form template
└── admin/                # Admin interface
```

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
1. ✅ Create templates in `src/web/templates/[feature]/`
2. ✅ Process data in Rust handlers (not templates)
3. ✅ Check existing CSS classes first
4. ✅ Add styles using CSS variables
5. ✅ Add minimal JavaScript if needed
6. ✅ Test without JavaScript

### Code Review Checklist
- [ ] No inline CSS styles
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
