import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import type { Vox } from "../model/vox";
import type { VoxRequestHandler } from "../model/protocol";

type VoxState = {
  state: "idle" | "connecting" | "live";
  error: string | null;
  toggle: () => void;
  dismiss: () => void;
};
const Context = createContext<VoxState | null>(null);
export const useVox = () => useContext(Context);

/** The workspace owns the call, independently of its titlebar and active chat. */
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
  const generation = useRef(0);
  const executor = useRef(execute);
  executor.current = execute;
  const [state, setState] = useState<VoxState["state"]>("idle");
  const [error, setError] = useState<string | null>(null);
  function stop() {
    generation.current++;
    const current = connection.current;
    connection.current = null;
    void current?.stop();
    setState("idle");
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
      document.removeEventListener("visibilitychange", hidden);
      window.removeEventListener("pagehide", leaving);
    };
  }, []);
  async function toggle() {
    if (state !== "idle") {
      stop();
      return;
    }
    const token = ++generation.current;
    setError(null);
    setState("connecting");
    try {
      const [{ Vox }, directory] = await Promise.all([
        import("../model/vox"),
        typeof cwd === "function" ? cwd() : cwd,
      ]);
      if (generation.current !== token) return;
      const call = new Vox({
        cwd: directory,
        execute: (request) => executor.current(request),
        getContext: (request) => executor.current(request),
        onConnected: () => {
          if (generation.current === token) setState("live");
        },
        onError: (message) => {
          if (generation.current !== token) return;
          connection.current = null;
          setState("idle");
          setError(message);
        },
      });
      connection.current = call;
      await call.start();
    } catch {
      if (generation.current !== token) return;
      stop();
      setError("Could not load Vox. Restart vorkspace and try again.");
    }
  }
  return (
    <Context.Provider
      value={{
        state,
        error,
        toggle: () => void toggle(),
        dismiss: () => setError(null),
      }}
    >
      {children}
    </Context.Provider>
  );
}
