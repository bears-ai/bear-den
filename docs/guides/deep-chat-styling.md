# Deep Chat styling

The bear chat UI uses the [Deep Chat](https://deepchat.dev) web component (`<deep-chat>`).

## References

- Styling docs: <https://deepchat.dev/docs/styles/>
- Design examples, including the full-width input pattern: <https://deepchat.dev/examples/design/>

## Route and template

| Route | Source |
|-------|--------|
| `GET /bear/{slug}` | `src/web/templates/bear_chat.html` |

The chat page extends `base.html` and loads the shared `style.css` stack. The Deep Chat bundle is vendored at `src/web/assets/deep-chat/deepChat.bundle.js` and served as `/assets/deep-chat/deepChat.bundle.js`.

## Key Deep Chat style properties

| JS property | What it controls |
|---|---|
| `style` / `chatStyle` | Host element / outer container |
| `messageStyles` | Per-role message layout, colours, borders, spacing, alignment |
| `textInput` | Input box styling, placeholder, character limit |
| `submitButtonStyles` | Send, loading, stop, and disabled button states |
| `inputAreaStyle` | Bottom bar wrapping input and buttons |
| `auxiliaryStyle` | Raw CSS injected into Deep Chat's shadow DOM |
| `avatars` | Avatar image/visibility by role |
| `names` | Name labels by role |
| `errorMessages` | Built-in error display behavior |

## Styling conventions

- Page chrome and layout live in `src/web/assets/css/specifics.css`, scoped to the bear chat page classes.
- Shared colours, spacing, typography, and chat bridge variables live in `src/web/assets/css/style.css` and imported CSS files.
- Prefer CSS variables such as `--page-color`, `--surface-color`, `--field-fill-color`, and `--chat-*` bridge variables.
- Keep the Den geometric design system: no rounded corners (`borderRadius: '0'`) and no gradients.
- The current chat layout is Slack-style: user and assistant messages are left-aligned with name labels and dividers rather than speech bubbles.
- The input uses the full-width input pattern: unset field border, top separator, and `width: '100%'`.

## Shadow DOM and token handling

Deep Chat styles are applied partly through JavaScript objects and partly inside its shadow DOM. Keep these rules in mind:

- `auxiliaryStyle` may use `var(--...)` for shadow-DOM CSS that should track theme tokens.
- For JS style objects such as `messageStyles`, `textInput`, and `submitButtonStyles`, pass resolved values from `getComputedStyle` rather than raw `calc(var(--...))` strings when a value must be computed in light DOM.
- `bear_chat.html` uses `resolveDeepChatLayoutTokens()` as a batched light-DOM probe for lengths that Deep Chat does not reliably resolve inside its shadow DOM.
- Avoid hard-coded hex colours and pixel sizes unless they are unavoidable third-party component glue.

## Behavior to preserve while styling

- `errorMessages.displayServiceErrorMessages: true` must be present in the `<deep-chat>` markup before Deep Chat builds its internal message stack; JavaScript may re-apply the same value later.
- The boot order should remain: load conversations, wait for `customElements.whenDefined('deep-chat')`, then configure Deep Chat.
- For streaming errors, send Deep Chat a `{ text, role: 'error' }` style event rather than `{ error: string }`; the latter can trigger a Deep Chat stream finalization path with no valid prior stream event.
- Assistant message chunks are accumulated across the whole response. Do not reset the assistant buffer merely because upstream stream IDs change.
- `stripSystemReminderBlocksLive` should only strip closed `<system-reminder>` blocks while streaming; a final pass can strip incomplete trailing blocks at EOF.

## Design fixture sync

Keep `src/web/templates/bear_chat.html` and `src/web/templates/design/chat.html` synchronized. When changing toolbar structure, conversation dropdown behavior, shell spacing, composer styling, message layout, or error rendering in one, update the other in the same change and verify `/design/chat` first.
