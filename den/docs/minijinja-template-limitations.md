# MiniJinja template limitations

MiniJinja is a Rust implementation of Jinja2 templates with significant limitations compared to full Python Jinja2. This document lists restrictions and patterns for templates in this project.

**Passing data from Rust:** see [`minijinja-context-patterns.md`](minijinja-context-patterns.md) (`minijinja::context!`).

## Critical limitations

### 1. Many built-in filters need named parameters

MiniJinja often requires **named** parameters for filters (unlike Jinja2 positional style):

```jinja2
<!-- ❌ WRONG - positional parameters don't work -->
{{ text | replace("old", "new") }}
{{ text | truncate(50) }}

<!-- ✅ CORRECT - use named parameters -->
{{ text | replace(from="old", to="new") }}
{{ text | truncate(length=50) }}
```

Yet `round` and `join` work best with positional parameters.

### 2. Unsupported filters

These common Jinja2 filters do not exist in MiniJinja:

- `format` — string formatting
- `selectattr` / `rejectattr` — filtering objects by attributes
- `map` with attribute parameter — extracting attributes from objects
- `unique` — removing duplicates
- `sum` with attribute parameter — summing object attributes

### 3. No numeric indexing of properties

```jinja2
<!-- ❌ BROKEN - numeric indexing not supported -->
{{ my_array.0 }}
{{ my_tuple.1 }}
{{ my_object.0 }}

<!-- ✅ CORRECT - use named properties or loop variables -->
{{ my_object.first_item }}
{{ my_object.second_item }}
{% for item in my_array %}
  {{ loop.index0 }}: {{ item }}
{% endfor %}
```

### 4. Limited object / dict methods

```jinja2
<!-- ❌ BROKEN - methods not available -->
{{ my_dict.get("key", "default") }}
{{ my_dict.items() }}
{{ my_dict.keys() }}
{{ my_list.append(item) }}
```

### 5. No complex filter chains

```jinja2
<!-- ❌ BROKEN - complex chains not supported -->
{{ items | selectattr("active") | map(attribute="name") | unique | sort }}
```

## Best practices

### Handler-side processing

Keep templates simple; compute and filter in Axum handlers, then pass lists and scalars via `context!`.

```rust
// Process in handler, then render
let total = raw_data.len() as f64;
let mut active_items: Vec<_> = raw_data
    .into_iter()
    .filter(|item| item.is_active)
    .map(|item| ProcessedItem {
        name: item.name,
        count: item.count,
        percentage: if total > 0.0 {
            (item.count as f64 / total) * 100.0
        } else {
            0.0
        },
    })
    .collect();
active_items.sort_by(|a, b| a.name.cmp(&b.name));

web::render_template(state.template_env, "my_template.html", auth_session, context! {
    items => active_items,
    total_count => active_items.len(),
})
.await?;
```

```jinja2
<h2>{{ total_count }} Active Items</h2>
{% for item in items %}
  <div>{{ item.name }}: {{ item.percentage | round(precision=1) }}%</div>
{% endfor %}
```

### Primitive context data

Prefer small `Serialize` structs and primitives over deep nested maps you would mutate in Jinja2.

### Validation errors

Build displayable error lists in the handler; templates only iterate or join.

```rust
let field_errors: Vec<String> = validation_errors
    .field_errors()
    .get("field_name")
    .map(|errors| {
        errors
            .iter()
            .map(|e| {
                e.message
                    .as_ref()
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "unknown error".to_string())
            })
            .collect()
    })
    .unwrap_or_default();

context! {
    field_errors => field_errors,
}
```

```jinja2
{% if field_errors %}
  <span class="error">{{ field_errors | join(sep=", ") }}</span>
{% endif %}
```

## Common patterns

**Percentage in handler, round in template:**

```rust
let percentage = if total > 0 {
    (count as f64 / total as f64) * 100.0
} else {
    0.0
};
```

```jinja2
{{ item.percentage | round(precision=1) }}%
```

**Counts in handler:**

```rust
let active_count = items.iter().filter(|item| item.is_active).count();
let inactive_count = items.len() - active_count;
```

**Aggregates as a sorted list of tuples** (e.g. categories) — build `Vec<(String, i64)>` in Rust, then:

```jinja2
{% for category_data in analysis %}
  <div>{{ category_data.0 }}: {{ category_data.1 }}</div>
{% endfor %}
```

## Supported filters (examples)

Safe when used with correct syntax:

- `round(precision=n)`
- `title`
- `replace(from="old", to="new")`
- `join(sep="separator")`
- `length`
- `pluralize`
- `safe`
- `truncate(length=n)`
- `upper`, `lower`

Avoid expecting `format`, `selectattr`, `rejectattr`, attribute `map`, `unique`, and attribute `sum` — do that work in Rust.

## Development workflow

After template changes:

```bash
cargo check --message-format=short
```

Runtime clues:

- `unknown filter: …`
- `invalid number of arguments` — often missing named parameters

**Debugging:** move the failing logic into the matching handler, pass precomputed values, simplify template to field access and loops.

## Code review

- Unsupported or chained filters
- Calculations that belong in handlers
- Dict/object methods in templates
- Missing named parameters on filters that require them

This keeps templates compatible with MiniJinja and easier to test (logic stays in Rust).
