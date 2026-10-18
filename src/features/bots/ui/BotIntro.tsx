import { playCue } from "../../settings/model/sounds";

type Props = {
  /** Creates the first Bot and opens it. */
  onCreate: () => void;
  /** Turns Bots off, as Settings › Bots › Show bots does. */
  onHide: () => void;
};

/**
 * Stands in for the rail's Bots while the user has none, inviting them to
 * make their first. The switch hides Bots altogether.
 */
export function BotIntroCard({ onCreate, onHide }: Props) {
  return (
    <section
      aria-label="Meet Bots"
      data-bot-intro
      className="mx-2 overflow-hidden rounded-lg bg-content/[0.03]"
    >
      <div className="flex items-center gap-2 pl-2.5 pr-2 pt-2">
        <h3 className="min-w-0 flex-1 truncate text-[13px] font-medium text-content">
          Meet Bots
        </h3>
        <button
          type="button"
          role="switch"
          aria-checked
          aria-label="Show bots"
          title="Hide bots from the rail. Turn them back on in Settings › Bots."
          onClick={() => {
            onHide();
            playCue("switch");
          }}
          className="relative h-4 w-7 shrink-0 rounded-full bg-accent transition-colors"
        >
          <span className="absolute left-3.5 top-0.5 size-3 rounded-full bg-white" />
        </button>
      </div>
      <p className="px-2.5 pt-1.5 text-[11px] leading-snug text-content/55">
        Agents of your own that work across your projects and run habits on
        their own.
      </p>

      <div className="p-2">
        <button
          type="button"
          onClick={onCreate}
          className="h-7 w-full cursor-default rounded-md bg-content text-[12px] font-medium text-background-base hover:bg-content/85 active:scale-[0.98]"
        >
          Create bot
        </button>
      </div>
    </section>
  );
}
