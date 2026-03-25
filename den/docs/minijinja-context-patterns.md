# MiniJinja Context Macro Patterns

This document explains the correct patterns for using the `minijinja::context!` macro in this project, including syntax, nested contexts, merging, and common pitfalls.

## Overview

The `minijinja::context!` macro creates a `minijinja::Value` that can be passed to templates. It's the primary way to pass data from Rust handlers to MiniJinja templates.

## Basic Syntax

### Simple Context

**Pattern**:
```rust
use minijinja::context;

let ctx = context! {
    username => "alice",
    count => 42,
    is_active => true,
};
```

**Template usage**:
```jinja2
<p>User: {{ username }}</p>
<p>Count: {{ count }}</p>
{% if is_active %}
  <span>Active</span>
{% endif %}
```

### Key Syntax Rules

1. **Arrow syntax**: Use `=>` to separate key from value
2. **No quotes on keys**: Keys are identifiers, not strings
3. **Comma-separated**: Each key-value pair separated by comma
4. **Trailing comma allowed**: Last item can have a trailing comma

**✅ CORRECT**:
```rust
context! {
    name => "value",
    count => 42,
}
```

**❌ WRONG**:
```rust
context! {
    "name" => "value",  // Keys don't need quotes
    count: 42,          // Use => not :
}
```

## Common Patterns

### Pattern 1: Simple Variables

```rust
web::render_template(
    state.template_env,
    "template.html",
    auth_session,
    context! {
        title => "Dashboard",
        user_count => 150,
        is_admin => true,
    },
)
.await
```

### Pattern 2: Using Existing Variables

```rust
let user_name = user.username.clone();
let item_count = summary.item_count;

context! {
    username => user_name,      // Use existing variable
    item_count,                 // Shorthand: key name matches variable name
    page_title,                 // Shorthand for page_title => page_title
}
```

**Shorthand syntax**: If the key name matches a variable name, you can omit the value:
```rust
let page_title = "Account".to_string();

// These are equivalent:
context! {
    page_title => page_title,
}

context! {
    page_title,  // Shorthand
}
```

### Pattern 3: Nested Contexts

**Pattern**: Use nested `context!` macros for structured data.

```rust
context! {
    summary => context! {
        total => summary.total,
        pending => summary.pending,
        completed => summary.completed,
    },
    daily => daily_rows,
}
```

**Template usage**:
```jinja2
<div>Total: {{ summary.total }}</div>
<div>Pending: {{ summary.pending }}</div>
<div>Completed: {{ summary.completed }}</div>
```

### Pattern 4: Merging Contexts

**Pattern**: Use `..` to merge an existing context into a new one.

```rust
// Existing context
let base_ctx = context! {
    username => config.username,
    display_name => config.display_name,
};

// Merge with additional fields
let merged_ctx = context! {
    base_url => base_url,
    ..base_ctx.clone(),  // Merge existing context
};
```

**Or merge from a function parameter**:
```rust
pub async fn send_email_template(
    template_env: Environment<'static>,
    template_name: &str,
    ctx: minijinja::Value,  // Existing context
) -> Result<(), CustomError> {
    let merged_ctx = minijinja::context! {
        base_url => base_url,
        username => config.username,
        display_name => config.display_name,
        ..ctx.clone()  // Merge parameter context
    };

    // Use merged_ctx...
}
```

**Important**: The `..` syntax merges contexts, with later values overriding earlier ones.

### Pattern 5: Complex Data Structures

**Pattern**: Pass serializable structs directly.

```rust
#[derive(serde::Serialize)]
struct WeekSummary {
    total_cells: i32,
    new_cells: i32,
    fresh_cells: i32,
}

let summary = WeekSummary {
    total_cells: 100,
    new_cells: 10,
    fresh_cells: 5,
};

context! {
    week_summary => summary,  // Struct automatically serialized
}
```

**Template usage**:
```jinja2
<div>Total: {{ week_summary.total_cells }}</div>
```

### Pattern 6: Vectors and Arrays

```rust
let items = vec!["item1", "item2", "item3"];

context! {
    items => items,
}
```

**Template usage**:
```jinja2
{% for item in items %}
  <div>{{ item }}</div>
{% endfor %}
```

### Pattern 7: Conditional Context Fields

```rust
let mut ctx = context! {
    title => "Dashboard",
    user_count => 150,
};

// Add conditional fields
if is_admin {
    ctx = context! {
        ..ctx.clone(),
        admin_panel => true,
        sensitive_data => data,
    };
}
```

**Or use a match**:
```rust
let merged_ctx = match auth_session.user {
    Some(user) => minijinja::context! {
        template_tag => template_tag,
        session => minijinja::context! {
            user_id => user.id,
            username => user.username,
            is_admin => user.is_admin,
            theme => user.theme,
        },
        ..ctx
    },
    None => ctx,
};
```

## Real-World Examples

### Example 1: Account / settings-style page

```rust
web::render_template(
    state.template_env,
    "account/view.html",
    auth_session,
    context! {
        user => context! {
            id => user.id,
            username => user.username.clone(),
            display_name => user.display_name.clone(),
            created => user.created_at,
        },
        stats => context! {
            open_tasks => stats.open,
            done_tasks => stats.done,
        },
    },
)
.await
```

### Example 2: Email template context

```rust
let context = context! {
    base_url => base_url,
    username => recipient_username,
    display_name => recipient_display_name,
    action_url => verify_link,
    // add only what the HTML template reads
};
```

### Example 3: Admin Dashboard

```rust
context! {
    template_tag => "admin-dashboard",
    stats => context! {
        total_users => user_count,
        active_sources => source_count,
        recent_errors => error_count,
    },
    recent_activities => activities,
}
```

## Common Pitfalls

### Pitfall 1: Forgetting to Clone

**❌ WRONG** - Moves ownership:
```rust
let base_ctx = context! { name => "value" };
let merged = context! {
    ..base_ctx,  // Moves base_ctx, can't use it again
};
```

**✅ CORRECT** - Clone when merging:
```rust
let base_ctx = context! { name => "value" };
let merged = context! {
    ..base_ctx.clone(),  // Clone to keep base_ctx usable
};
```

### Pitfall 2: Wrong Arrow Direction

**❌ WRONG**:
```rust
context! {
    name: "value",  // Wrong: use => not :
}
```

**✅ CORRECT**:
```rust
context! {
    name => "value",
}
```

### Pitfall 3: Quoting Keys

**❌ WRONG**:
```rust
context! {
    "username" => user.name,  // Keys don't need quotes
}
```

**✅ CORRECT**:
```rust
context! {
    username => user.name,
}
```

### Pitfall 4: Complex Expressions in Context

**❌ WRONG** - Complex logic in context macro:
```rust
context! {
    percentage => if total > 0 { (count / total) * 100 } else { 0 },  // Too complex
}
```

**✅ CORRECT** - Calculate first, then pass:
```rust
let percentage = if total > 0 {
    (count as f64 / total as f64) * 100.0
} else {
    0.0
};

context! {
    percentage,
}
```

### Pitfall 5: Nested Context Syntax

**❌ WRONG** - Missing nested context macro:
```rust
context! {
    user => {  // Wrong: need context! macro
        id => user.id,
    },
}
```

**✅ CORRECT** - Use nested context!:
```rust
context! {
    user => context! {
        id => user.id,
    },
}
```

## Integration with render_template

The `web::render_template` function signature:
```rust
pub async fn render_template(
    template_env: Environment<'static>,
    template_id: &str,
    auth_session: AuthSession<Backend>,
    ctx: minijinja::Value,  // This is what context! creates
) -> Result<Response, CustomError>
```

**Usage**:
```rust
web::render_template(
    state.template_env,
    "my_template.html",
    auth_session,
    context! {
        title => "My Page",
        data => my_data,
    },
)
.await
```

**Note**: The `render_template` function automatically adds session data if a user is logged in. You don't need to manually add `session` to your context unless you're overriding it.

## Type Compatibility

The `context!` macro accepts:
- **Primitives**: `i32`, `i64`, `f64`, `bool`, `String`, `&str`
- **Collections**: `Vec<T>`, `HashMap<K, V>` (if `T`, `K`, `V` are serializable)
- **Structs**: Any type that implements `serde::Serialize`
- **Nested contexts**: Other `minijinja::Value` objects
- **Options**: `Option<T>` where `T` is serializable

**Example with Option**:
```rust
context! {
    username => user.username.clone(),
    email => user.email.clone(),  // Option<String> works fine
    last_login => user.last_login,  // Option<DateTime> works fine
}
```

**Template usage with Option**:
```jinja2
{% if email %}
  <p>Email: {{ email }}</p>
{% endif %}
```

## Best Practices

### 1. Keep Contexts Simple

**✅ CORRECT** - Pre-process data:
```rust
let processed_items: Vec<ProcessedItem> = raw_items
    .into_iter()
    .map(|item| ProcessedItem {
        name: item.name,
        percentage: calculate_percentage(&item),
    })
    .collect();

context! {
    items => processed_items,
}
```

### 2. Use Descriptive Key Names

**✅ CORRECT**:
```rust
context! {
    week_summary => summary,
    user_profile => profile,
    extra_json => json_string,
}
```

### 3. Group Related Data

**✅ CORRECT** - Group related fields:
```rust
context! {
    user => context! {
        id => user.id,
        username => user.username,
        email => user.email,
    },
    stats => context! {
        item_count => stats.item_count,
        last_updated => stats.last_updated,
    },
}
```

### 4. Document Complex Contexts

For complex contexts, add a comment:
```rust
// Context structure:
// - user: User profile data
// - stats: Summary numbers for the page
// - extra_json: Optional serialized blob for the client
let ctx = context! {
    user => context! { /* ... */ },
    stats => context! { /* ... */ },
    extra_json => json,
};
```

## Related Documentation

- [minijinja-template-limitations.md](minijinja-template-limitations.md) — what works in templates vs full Jinja2
- [concepts-overview.md](concepts-overview.md) — repository layout (`src/web/` vs `src/api/`)
- Template examples: `src/web/templates/`

