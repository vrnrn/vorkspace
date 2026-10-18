import { useLayoutEffect, useRef, type RefObject } from "react";
import { registerComposerEditor } from "../model/composerControl";

export function useComposerControl(options: {
  sessionId?: string;
  enabled: boolean;
  field: RefObject<HTMLTextAreaElement | null>;
  write: (text: string) => void;
}) {
  const latest = useRef(options.write);
  useLayoutEffect(() => {
    latest.current = options.write;
  });
  useLayoutEffect(() => {
    const field = options.field.current;
    if (!options.enabled || !options.sessionId || !field) return;
    return registerComposerEditor(options.sessionId, {
      field,
      write: (text) => latest.current(text),
    });
  }, [options.sessionId, options.enabled, options.field]);
}
