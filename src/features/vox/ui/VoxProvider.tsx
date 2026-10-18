import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import type { Vox } from "../model/vox";
import { VoxDisplay } from "../model/display";
import type { VoxRequestHandler } from "../model/protocol";

type Phase = "idle" | "connecting" | "live";
export type VoxControls = {
  state: Phase;
  error: string | null;
  display: VoxDisplay;
  silent: boolean;
  micEnabled: boolean;
  start: () => void;
  stop: () => void;
  toggleSilent: () => void;
  toggleMic: () => void;
};
const Context = createContext<VoxControls | null>(null);
export const useVox = () => useContext(Context);

/** The workspace owns the call; the mounted panel alone observes captions. */
export function VoxProvider({
  cwd,
  execute,
  children,
}: {
  cwd: string | (() => Promise<string>);
  execute: VoxRequestHandler;
  children: ReactNode;
}) {
  const connection = useRef<Vox | null>(null);
  const stopping = useRef<Promise<void> | undefined>(undefined);
  const generation = useRef(0);
  const phase = useRef<Phase>("idle");
  const preferences = useRef({ silent: false, micEnabled: true });
  const executor = useRef(execute);
  executor.current = execute;
  const [display] = useState(() => new VoxDisplay());
  const [state, setState] = useState<Phase>("idle");
  const [error, setError] = useState<string | null>(null);
  const [silent, setSilent] = useState(false);
  const [micEnabled, setMicEnabled] = useState(true);
  function setPhase(next: Phase) {
    phase.current = next;
    setState(next);
  }
  function stop() {
    generation.current++;
    const current = connection.current;
    connection.current = null;
    if (current) stopping.current = current.stop();
    display.finish();
    setPhase("idle");
  }
  useEffect(() => {
    const hidden = () => {
      if (document.hidden) stop();
    };
    const leaving = () => stop();
    document.addEventListener("visibilitychange", hidden);
    window.addEventListener("pagehide", leaving);
    return () => {
      generation.current++;
      void connection.current?.stop();
      connection.current = null;
      display.dispose();
      document.removeEventListener("visibilitychange", hidden);
      window.removeEventListener("pagehide", leaving);
    };
  }, [display]);
  async function start() {
    if (phase.current !== "idle") return;
    const token = ++generation.current;
    setError(null);
    setPhase("connecting");
    preferences.current.micEnabled = true;
    setMicEnabled(true);
    display.reset();
    try {
      const [{ Vox }, directory] = await Promise.all([
        import("../model/vox"),
        typeof cwd === "function" ? cwd() : cwd,
        // Old spawn/kill admission must settle before a successor owns audio.
        stopping.current,
      ]);
      if (generation.current !== token) return;
      const call = new Vox({
        cwd: directory,
        ...preferences.current,
        display,
        execute: (request) => executor.current(request),
        getContext: (request) => executor.current(request),
        onConnected: () => {
          if (generation.current === token) setPhase("live");
        },
        onError: (message) => {
          if (generation.current !== token) return;
          stopping.current = call.stop();
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
      stop();
      setError("could not load vox. restart vorkspace and try again.");
    }
  }
  return (
    <Context.Provider
      value={{
        state,
        error,
        display,
        silent,
        micEnabled,
        start: () => void start(),
        stop,
        toggleSilent: () => {
          const next = !preferences.current.silent;
          preferences.current.silent = next;
          connection.current?.setSilent(next);
          setSilent(next);
        },
        toggleMic: () => {
          const next = !preferences.current.micEnabled;
          preferences.current.micEnabled = next;
          connection.current?.setMicEnabled(next);
          setMicEnabled(next);
        },
      }}
    >
      {children}
    </Context.Provider>
  );
}
