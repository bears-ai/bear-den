# Template Development Guide

## Important Guidelines

### 🚫 What NOT to do in templates
- **Never use inline CSS styles** - use CSS classes instead
- **Don't put complex logic in templates** - process data in Rust handlers
- **Avoid calculations and data transformations** - do this server-side

### ✅ What TO do in templates
- **Use semantic HTML** for accessibility
- **Use existing CSS classes** from `src/web/assets/css/`
- **Keep templates simple** - just display processed data
- **Use MiniJinja template inheritance** with `extends` and `block`

## MiniJinja Limitations

⚠️ **Critical**: MiniJinja is NOT full Jinja2. See [`docs/minijinja-template-limitations.md`](../../../docs/minijinja-template-limitations.md) for restrictions and patterns.

### Common Issues
- Many filters require **named parameters**: `{{ value | round(precision=2) }}`
- No dictionary methods like `.get()`, `.items()`, `.keys()`
- Limited filter support - process complex data in handlers
- No numeric indexing - use named properties instead

## Template Structure

```
src/web/templates/
├── base.html              # Base layout - extend this
├── forms.jinja           # Form macros and helpers
├── [feature]/            # Feature-specific templates
│   ├── view.html        # Display template
│   ├── edit.html        # Form template
│   └── list.html        # List template
└── admin/               # Admin interface templates
```

## Best Practices

### Data Processing
```rust
// ✅ GOOD - Process in handler
pub async fn my_handler() -> Result<Response, CustomError> {
    let raw_data = get_data().await?;
    
    // Calculate here, not in template
    let processed_data: Vec<ProcessedItem> = raw_data
        .into_iter()
        .map(|item| ProcessedItem {
            name: item.name,
            percentage: calculate_percentage(item.count, total),
            is_new: item.created_at > recent_threshold,
        })
        .collect();
    
    render_template("my_template.html", context! {
        items => processed_data,
    })
}
```

```html
<!-- ✅ GOOD - Simple template -->
{% for item in items %}
  <div class="item {% if item.is_new %}new-item{% endif %}">
    {{ item.name }}: {{ item.percentage }}%
  </div>
{% endfor %}
```

### CSS Classes
```html
<!-- ✅ GOOD - Use existing classes -->
<form class="fields-grid">
  <label>Name</label>
  <div class="form-field">
    <input type="text" name="name">
  </div>
</form>

<!-- ❌ BAD - Inline styles -->
<div style="padding: 10px; border: 1px solid black;">
  Content
</div>
```

### Error Handling
```rust
// ✅ GOOD - Process errors in handler
let field_errors: Vec<String> = validation_errors
    .field_errors()
    .get("name")
    .map(|errors| errors.iter().map(|e| e.to_string()).collect())
    .unwrap_or_default();

context! {
    field_errors => field_errors,
}
```

```html
<!-- ✅ GOOD - Simple error display -->
{% if field_errors %}
  <span class="error">{{ field_errors | join(sep=", ") }}</span>
{% endif %}
```

## Template Patterns

### Form Templates
- Extend `forms.jinja` for form helpers
- Use `.fields-grid` layout from `common.css`
- Process validation errors in handlers

### List Templates  
- Use existing grid/list classes
- Process filtering/sorting in handlers
- Include pagination data from handlers

### Detail Templates
- Use semantic HTML structure
- Include breadcrumbs and navigation
- Process related data in handlers

## Development Workflow

1. **Create handler** - process all data logic
2. **Design template** - focus on HTML structure
3. **Apply CSS classes** - use existing styles first
4. **Test functionality** - ensure it works without JS
5. **Add progressive enhancement** - optional JavaScript

## Quick Reference

- **CSS Classes**: Check `src/web/assets/css/` files
- **Form Helpers**: See `forms.jinja` 
- **Base Layout**: Extend `base.html`
- **MiniJinja limits**: [`docs/minijinja-template-limitations.md`](../../../docs/minijinja-template-limitations.md)
- **Handler Examples**: Look at existing handlers in `src/web/`

**Remember**: When in doubt, process the data in Rust and keep templates simple!