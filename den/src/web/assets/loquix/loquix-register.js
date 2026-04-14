/**
 * Register Loquix custom elements from the CDN IIFE bundle.
 *
 * The cdn/loquix.min.js bundle exports classes on `window.Loquix` but does NOT
 * call customElements.define(). This script bridges that gap so that tags like
 * <loquix-chat-container> upgrade to the correct Lit class.
 *
 * @loquix/core 0.1.2 — keep in sync with VERSION.
 */
(function () {
  var L = window.Loquix;
  if (!L) {
    console.error("Loquix CDN bundle not loaded before loquix-register.js");
    return;
  }
  var map = {
    "loquix-action-button":       L.LoquixActionButton,
    "loquix-action-copy":         L.LoquixActionCopy,
    "loquix-action-edit":         L.LoquixActionEdit,
    "loquix-action-feedback":     L.LoquixActionFeedback,
    "loquix-attachment-chip":     L.LoquixAttachmentChip,
    "loquix-attachment-panel":    L.LoquixAttachmentPanel,
    "loquix-caveat-notice":       L.LoquixCaveatNotice,
    "loquix-chat-composer":       L.LoquixChatComposer,
    "loquix-chat-container":      L.LoquixChatContainer,
    "loquix-chat-header":         L.LoquixChatHeader,
    "loquix-composer-toolbar":    L.LoquixComposerToolbar,
    "loquix-disclosure-badge":    L.LoquixDisclosureBadge,
    "loquix-dropdown-select":     L.LoquixDropdownSelect,
    "loquix-example-gallery":     L.LoquixExampleGallery,
    "loquix-filter-bar":          L.LoquixFilterBar,
    "loquix-follow-up-suggestions": L.LoquixFollowUpSuggestions,
    "loquix-generation-controls": L.LoquixGenerationControls,
    "loquix-message-actions":     L.LoquixMessageActions,
    "loquix-message-avatar":      L.LoquixMessageAvatar,
    "loquix-message-content":     L.LoquixMessageContent,
    "loquix-message-item":        L.LoquixMessageItem,
    "loquix-message-list":        L.LoquixMessageList,
    "loquix-mode-selector":       L.LoquixModeSelector,
    "loquix-model-selector":      L.LoquixModelSelector,
    "loquix-nudge-banner":        L.LoquixNudgeBanner,
    "loquix-parameter-panel":     L.LoquixParameterPanel,
    "loquix-prompt-input":        L.LoquixPromptInput,
    "loquix-suggestion-chips":    L.LoquixSuggestionChips,
    "loquix-template-card":       L.LoquixTemplateCard,
    "loquix-template-picker":     L.LoquixTemplatePicker,
    "loquix-typing-indicator":    L.LoquixTypingIndicator,
    "loquix-welcome-screen":      L.LoquixWelcomeScreen,
  };
  Object.keys(map).forEach(function (tag) {
    if (map[tag] && !customElements.get(tag)) {
      customElements.define(tag, map[tag]);
    }
  });
})();
