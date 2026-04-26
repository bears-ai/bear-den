/**
 * Placeholder registry for Slack (etc.) channel listeners running alongside conversation handlers.
 * Wire real listeners here without changing Den’s pool API shape.
 */
export type ChannelListenerRegistry = {
  stats: () => {
    kind: "channel_listener";
    status: "stub" | "running";
    listeners: Array<{ id: string; channel: string; note?: string }>;
  };
};

export function createChannelListenerRegistry(): ChannelListenerRegistry {
  return {
    stats() {
      return {
        kind: "channel_listener",
        status: "stub",
        listeners: [],
      };
    },
  };
}
