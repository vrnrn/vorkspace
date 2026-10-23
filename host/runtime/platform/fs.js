import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { mkdir, open, unlink } from "node:fs/promises";
import { randomUUID } from "node:crypto";
let imageDirectory;
const pendingImages = new Set();
const MAX_IMAGE_BYTES = 20 * 1024 * 1024;
/** The host owns image persistence; callers never supply a destination path. */
export function configureHostFiles(dataDirectory) {
  if (imageDirectory) throw new Error("Host files are already configured");
  imageDirectory = join(resolve(dataDirectory), "generated-images");
}
export async function homeDir() {
  return homedir();
}
export async function saveGeneratedImage({ data }) {
  if (!imageDirectory) throw new Error("Host files are not configured");
  if (typeof data !== "string" || data.length > MAX_IMAGE_BYTES * 1.5)
    throw new Error("Generated image is too large");
  const encoded = data.replace(/\s/g, "");
  if (
    encoded.length % 4 !== 0 ||
    !/^[A-Za-z0-9+/]*={0,2}$/.test(encoded)
  )
    throw new Error("Generated image data is not valid base64");
  const bytes = Buffer.from(encoded, "base64");
  if (!bytes.length || bytes.length > MAX_IMAGE_BYTES)
    throw new Error("Generated image is too large");
  if (
    !bytes.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))
  )
    throw new Error("Generated image data is not a PNG image");
  await mkdir(imageDirectory, { recursive: true, mode: 0o700 });
  const path = join(imageDirectory, `${randomUUID()}.png`);
  const file = await open(path, "wx", 0o600);
  try {
    await file.writeFile(bytes);
    await file.sync();
  } catch (error) {
    await file.close();
    await unlink(path).catch(() => {});
    throw error;
  }
  await file.close();
  pendingImages.add(path);
  return { path, mimeType: "image/png", size: bytes.length };
}
/** Only unpublished files issued by this process can be canceled. */
export async function deleteGeneratedImages(paths) {
  for (const path of paths) {
    if (!pendingImages.has(path))
      throw new Error("Invalid generated image path");
    await unlink(path).catch((error) => {
      if (error.code !== "ENOENT") throw error;
    });
    pendingImages.delete(path);
  }
}
export function acceptGeneratedImage(path) {
  pendingImages.delete(path);
}
