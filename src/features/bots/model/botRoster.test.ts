// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  addBotProject,
  createBot,
  findBot,
  listBots,
  botLook,
  botProjectsPhrase,
  botWorksOn,
  removeBot,
  removeBotProject,
  reorderBots,
} from "./bot";
import { botBackgroundKey, botChatBackground } from "./botBackground";
import { saveProjectChatBackgroundSettings } from "../../projects/model/projectChatBackground";

beforeEach(() => {
  const stored = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => stored.get(key) ?? null,
    setItem: (key: string, value: string) => stored.set(key, value),
    removeItem: (key: string) => stored.delete(key),
  });
});
afterEach(() => vi.unstubAllGlobals());

it("gives each new Bot a mascot and color the others are not using", () => {
  const first = createBot();
  const second = createBot();
  expect(second.mascot).not.toBe(first.mascot);
  expect(second.color).not.toBe(first.color);
  expect(botLook(first).name).toMatch(/^Bot[A-Z]/);
  expect(first.projects).toEqual([]);
});

it("adds each project once and takes one away by its path", () => {
  const { id } = createBot(["/code/app"]);
  addBotProject(id, "/code/site");
  addBotProject(id, "/code/app/");
  expect(findBot(id)?.projects).toEqual(["/code/app", "/code/site"]);
  expect(botWorksOn(findBot(id)!, "/code/site/")).toBe(true);
  removeBotProject(id, "/code/app/");
  expect(findBot(id)?.projects).toEqual(["/code/site"]);
});

it("keeps the rail's order and forgets a removed Bot", () => {
  const a = createBot();
  const b = createBot();
  const c = createBot();
  reorderBots([c.id, a.id, b.id]);
  expect(listBots().map((bot) => bot.id)).toEqual([c.id, a.id, b.id]);
  removeBot(a.id);
  expect(listBots().map((bot) => bot.id)).toEqual([c.id, b.id]);
});

it("names its projects the way a sentence would", () => {
  const project = (name: string) => ({ path: `/code/${name}`, name });
  expect(botProjectsPhrase([])).toBe("");
  expect(botProjectsPhrase([project("app")])).toBe("app");
  expect(
    botProjectsPhrase([project("app"), project("site"), project("api")]),
  ).toBe("app, site and api");
});

it("forgets its background along with the Bot", () => {
  const bot = createBot();
  saveProjectChatBackgroundSettings(botBackgroundKey(bot.id), {
    path: "/bg/bot.png",
    emptyOpacity: 0.24,
    sessionOpacity: 0.24,
    scope: "all",
    effect: "gradient-blur",
  });
  expect(botChatBackground(bot.id)?.path).toBe("/bg/bot.png");
  removeBot(bot.id);
  expect(botChatBackground(bot.id)).toBeNull();
});
