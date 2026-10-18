import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import type { Vox } from "../model/vox";
import { loadVoxEnabled, subscribeVoxEnabled } from "../model/availability";
import { VoxDisplay } from "../model/display";
import type { VoxRequestHandler } from "../model/protocol";
import { loadVoxSilent, subscribeVoxSilent } from "../model/replyMode";
import { registerVoxCloser } from "../model/lifecycle";
import {
  resetSavedVoxConversation,
  VOX_CONVERSATION_KEY,
} from "../model/conversation";

type Phase = "idle" | "connecting" | "live";
export type VoxControls = {
  state: Phase;
  error: string | null;
  display: VoxDisplay;
  silent: boolean;
  resetConversation: () => Promise<void>;
  start: () => void;
  stop: () => void;
  end: () => Promise<void>;
};
const Context = createContext<VoxControls | null>(null);
export const useVox = () => useContext(Context);

type Props = {
  cwd: string | (() => Promise<string>);
  execute: VoxRequestHandler;
  children: ReactNode;
};

/** The workspace owns the call; titlebar controls observe its bounded display. */
export function VoxProvider({ cwd, execute, children }: Props) {
  const enabled = useSyncExternalStore(subscribeVoxEnabled, loadVoxEnabled);
  const connection = useRef<Vox | null>(null);
  const stopping = useRef<Promise<void> | undefined>(undefined);
  const generation = useRef(0);
  const phase = useRef<Phase>("idle");
  const executor = useRef(execute);
  executor.current = execute;
  const [display] = useState(() => new VoxDisplay());
  const [state, setState] = useState<Phase>("idle");
  const [error, setError] = useState<string | null>(null);
  const silent = useSyncExternalStore(subscribeVoxSilent, loadVoxSilent);
  useEffect(() => {
    connection.current?.setSilent(silent);
  }, [silent]);
  function setPhase(next: Phase) {
    phase.current = next;
    setState(next);
  }
  function rememberStop(saving: Promise<void>) {
    if (stopping.current !== saving) {
      stopping.current = saving;
      // Observe failures for the UI without replacing the rejecting promise
      // awaited by repeated End/close calls. Retire completed cleanup so a
      // reported failure cannot permanently lock the workspace open.
      void saving.then(
        () => {
          if (stopping.current === saving) stopping.current = undefined;
        },
        (error: unknown) => {
          if (stopping.current !== saving) return;
          stopping.current = undefined;
          if (phase.current !== "idle") return;
          setError(
            error instanceof Error
              ? error.message
              : "vox's latest speech could not be saved.",
          );
        },
      );
    }
    return saving;
  }
  function stop() {
    ++generation.current;
    const current = connection.current;
    connection.current = null;
    const saving = current?.stop() ?? stopping.current ?? Promise.resolve();
    rememberStop(saving);
    display.finish();
    setPhase("idle");
    return saving;
  }
  useEffect(() => {
    if (!enabled) {
      void stop().catch(() => undefined);
    }
  }, [enabled]);
  useEffect(() => {
    const releaseCloser = registerVoxCloser(stop);
    const hidden = () => {
      if (document.hidden) void stop().catch(() => undefined);
    };
    const leaving = () => {
      void stop().catch(() => undefined);
    };
    const changed = (event: StorageEvent) => {
      if (event.key !== VOX_CONVERSATION_KEY) return;
      const revision = (raw: string | null) => {
        try {
          return raw ? JSON.parse(raw).revision : "initial";
        } catch {
          return null;
        }
      };
      if (revision(event.oldValue) === revision(event.newValue)) return;
      void stop().catch(() => undefined);
      display.reset();
    };
    document.addEventListener("visibilitychange", hidden);
    window.addEventListener("pagehide", leaving);
    window.addEventListener("storage", changed);
    return () => {
      releaseCloser();
      generation.current++;
      void connection.current?.stop().catch(() => undefined);
      connection.current = null;
      display.dispose();
      document.removeEventListener("visibilitychange", hidden);
      window.removeEventListener("pagehide", leaving);
      window.removeEventListener("storage", changed);
    };
  }, [display]);
  async function start() {
    if (!loadVoxEnabled() || phase.current !== "idle") return;
    const token = ++generation.current;
    setError(null);
    setPhase("connecting");
    display.reset();
    try {
      const [{ Vox }, directory] = await Promise.all([
        import("../model/vox"),
        typeof cwd === "function" ? cwd() : cwd,
        // Old spawn/kill admission must settle before a successor owns audio.
        stopping.current?.catch(() => undefined),
      ]);
      if (generation.current !== token || !loadVoxEnabled()) return;
      stopping.current = undefined;
      const call = new Vox({
        cwd: directory,
        silent: loadVoxSilent(),
        display,
        execute: (request) => executor.current(request),
        getContext: (request) => executor.current(request),
        onConnected: () => {
          if (generation.current === token) setPhase("live");
        },
        onStopped: (saving) => {
          if (generation.current !== token) return;
          connection.current = null;
          setPhase("idle");
          display.finish();
          rememberStop(saving);
        },
        onError: (message) => {
          if (generation.current !== token) return;
          rememberStop(call.stop());
          connection.current = null;
          display.finish();
          setPhase("idle");
          setError(message.toLowerCase());
        },
      });
      connection.current = call;
      await call.start();
    } catch {
      if (generation.current !== token) return;
      void stop().catch(() => undefined);
      setError("could not load vox. restart vorkspace and try again.");
    }
  }
  const controls: VoxControls = {
    state,
    error,
    display,
    silent,
    resetConversation: () => {
      const closed = stop();
      const reset = (async () => {
        await closed.catch(() => undefined);
        resetSavedVoxConversation();
        display.reset();
        setError(null);
      })();
      // A concurrent click cannot resume the old conversation before reset.
      return rememberStop(reset);
    },
    start: () => void start(),
    stop: () => {
      void stop().catch(() => undefined);
    },
    end: stop,
  };
  return <Context.Provider value={controls}>{children}</Context.Provider>;
}
