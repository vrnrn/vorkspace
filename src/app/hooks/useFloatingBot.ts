import { useEffect, useRef, useSyncExternalStore } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { IS_MAC } from "../../platform/tauri/platform";
import type { Session } from "../../features/sessions/model/session";
import { findBot } from "../../features/bots/model/bot";
import {
  loadBotMenuBarIcon,
  subscribeBotMenuBarIcon,
} from "../../features/settings/model/settings";
import {
  deliverFloatingBotRequest,
  floatingBotRoster,
  floatingBotMenuMascots,
  floatingBotSession,
  FLOATING_BOT_REQUEST,
  type FloatingBotHost,
  type FloatingBotRequest,
} from "../../features/bots/model/floatingBot";

type Host = Omit<FloatingBotHost, "create"> & {
  /** Add a Bot to the roster and return its id. */
  create(): string;
};

/** The floating webview never boots providers or persists the live transcript. */
export function useFloatingBot(
  sessions: Session[],
  rosterKey: string,
  enabled: boolean,
  host: Host,
) {
  const current = useRef({ sessions, enabled, host });
  current.current = { sessions, enabled, host };
  const active = useRef(new Set<string>());
  const syncing = useRef(Promise.resolve());
  const refresh = useRef<() => void>(() => {});
  const publish = useRef<() => void>(() => {});

  useEffect(() => {
    if (!IS_MAC || !isTauri()) return;
    let disposed = false;
    let draining = false;
    let again = false;
    let unlisten: (() => void) | undefined;
    const lastSessions = new Map<string, Session>();
    const publishSession = (botId: string, fallback?: Session) => {
      const sessionId = findBot(botId)?.sessionId;
      const session =
        current.current.sessions.find((s) => s.id === sessionId) ?? fallback;
      if (!session) return;
      lastSessions.set(botId, session);
      void invoke("bot_chat_publish", {
        botId,
        session: floatingBotSession(session),
      }).catch(console.error);
    };
    publish.current = () => {
      for (const botId of active.current) {
        const bot = findBot(botId);
        if (!bot || !current.current.enabled) {
          active.current.delete(botId);
          lastSessions.delete(botId);
          continue;
        }
        const session = current.current.sessions.find(
          (s) => s.id === bot.sessionId,
        );
        if (session && session !== lastSessions.get(botId)) {
          publishSession(botId);
        }
      }
    };
    const drain = async () => {
      again = true;
      if (draining || disposed) return;
      draining = true;
      try {
        while (again && !disposed) {
          again = false;
          const requests =
            await invoke<FloatingBotRequest[]>("bot_chat_take");
          for (const request of requests) {
            if (disposed) break;
            let error: string | null = null;
            try {
              const session = await deliverFloatingBotRequest(
                request,
                { ...current.current.host, create },
                () =>
                  disposed
                    ? Promise.resolve(false)
                    : invoke<boolean>("bot_chat_accept", { id: request.id }),
              );
              if (session) {
                active.current.add(request.botId);
                publishSession(request.botId, session);
              }
            } catch (reason) {
              error = reason instanceof Error ? reason.message : String(reason);
            }
            await invoke("bot_chat_reply", { id: request.id, error });
          }
        }
      } finally {
        draining = false;
      }
    };
    const roster = () => {
      const bots = floatingBotRoster(
        current.current.enabled,
        current.current.sessions,
      );
      const hosted = bots.flatMap((bot) => {
        const session = current.current.sessions.find(
          (s) => s.id === bot.sessionId,
        );
        return session ? [{ botId: bot.id, busy: !!session.busy }] : [];
      });
      return { bots, hosted, mascots: floatingBotMenuMascots(bots) };
    };
    // The native side only opens Bots it has heard of. This runs inside a
    // drain, so it must not wait on the sync queue, which waits on the drain.
    const create = async (from: string) => {
      const to = current.current.host.create();
      await invoke("bot_chat_sync", roster());
      await invoke("bot_chat_switch", { from, to });
    };
    const sync = () => {
      const args = roster();
      syncing.current = syncing.current
        .catch(() => undefined)
        .then(async () => {
          if (disposed) return;
          await invoke("bot_chat_sync", args);
          await drain();
          publish.current();
        })
        .catch(console.error);
    };
    void getCurrentWebviewWindow()
      .listen(FLOATING_BOT_REQUEST, () => {
        void drain().catch(console.error);
      })
      .then((stop) => {
        if (disposed) {
          stop();
          return;
        }
        unlisten = stop;
        refresh.current = sync;
        sync();
      })
      .catch(console.error);
    return () => {
      disposed = true;
      refresh.current = () => {};
      publish.current = () => {};
      unlisten?.();
    };
  }, []);

  const roster = floatingBotRoster(enabled, sessions);
  const hostedKey = roster
    .map((m) => {
      const session = sessions.find((s) => s.id === m.sessionId);
      return `${m.sessionId}:${!!session?.busy}:${m.status}`;
    })
    .join("|");
  useEffect(() => {
    refresh.current();
  }, [rosterKey, enabled, hostedKey]);

  // The native side remembers this too; syncing here keeps it matching the
  // setting if the two ever drift.
  const menuBarIcon = useSyncExternalStore(
    subscribeBotMenuBarIcon,
    loadBotMenuBarIcon,
  );
  useEffect(() => {
    if (!IS_MAC || !isTauri()) return;
    void invoke("bot_menu_bar_set_visible", { visible: menuBarIcon }).catch(
      console.error,
    );
  }, [menuBarIcon]);

  // Follow the same React commits as the main transcript. A polling interval
  // batches streamed lines into visible jumps and delays the send entrance.
  // This also keeps publishing when a hidden owner has no animation frames.
  useEffect(() => {
    publish.current();
  }, [sessions]);
}
