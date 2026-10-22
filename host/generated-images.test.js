import { afterAll, expect, it } from "vitest";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  configureHostFiles,
  saveGeneratedImage,
  deleteGeneratedImages,
  acceptGeneratedImage,
  homeDir,
} from "./runtime/platform/fs.js";
const directory = await mkdtemp(join(tmpdir(), "vorkspace-host-images-"));
configureHostFiles(directory);
afterAll(() => rm(directory, { recursive: true, force: true }));
const png = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10, 1, 2, 3]);
it("persists generated images privately and cancels only files issued by this owner", async () => {
  const asset = await saveGeneratedImage({
    data: png.toString("base64"),
    name: "../../outside",
  });
  expect(asset.path.startsWith(join(directory, "generated-images"))).toBe(true);
  expect(await readFile(asset.path)).toEqual(png);
  expect(asset.mimeType).toBe("image/png");
  if (process.platform !== "win32")
    expect((await stat(asset.path)).mode & 0o777).toBe(0o600);
  await expect(
    deleteGeneratedImages([join(directory, "unrelated")]),
  ).rejects.toThrow("Invalid");
  await deleteGeneratedImages([asset.path]);
  await expect(stat(asset.path)).rejects.toMatchObject({ code: "ENOENT" });
});
it("keeps accepted images and rejects invalid or excessive input before writing", async () => {
  const asset = await saveGeneratedImage({ data: png.toString("base64") });
  acceptGeneratedImage(asset.path);
  await expect(deleteGeneratedImages([asset.path])).rejects.toThrow("Invalid");
  expect(await readFile(asset.path)).toEqual(png);
  for (const data of [
    "@@@",
    Buffer.from("not an image").toString("base64"),
    "x".repeat(32 * 1024 * 1024),
  ])
    await expect(saveGeneratedImage({ data })).rejects.toThrow();
  expect(await homeDir()).toBeTruthy();
});
it("persists a multi-megabyte image without overflowing the base64 validator", async () => {
  const bytes = Buffer.alloc(8 * 1024 * 1024);
  png.copy(bytes);
  const asset = await saveGeneratedImage({ data: bytes.toString("base64") });
  expect(asset.size).toBe(bytes.length);
  expect((await readFile(asset.path)).equals(bytes)).toBe(true);
  await deleteGeneratedImages([asset.path]);
});
