// SPDX-License-Identifier: GPL-2.0-or-later
//
// Pack folder access over the File System Access API (wt-7). The Rust
// `WebFileService` calls these from an async task and routes the results into
// its polled slots; the directory handles live here, in a JS-side map keyed by
// the opaque token Rust round-trips as a virtual `/<token>` path.
//
// Every operation mirrors the native pack file service (crates/vgms-app):
// the same `vgm/vgz/png/txt` filter and lowercase sort on scan, and the same
// rename decision tree (same-name no-op, case-only via a temp bounce, and
// existence-check-then-fail rather than overwrite).
//
// Test seam: the e2e harness installs `globalThis.__vgms_pick_dir(purpose)` to
// return an OPFS-backed directory handle instead of prompting, so the specs
// drive pack mode without a real OS picker. In production that global is unset
// and the true `showDirectoryPicker` runs.

const PACK_EXTENSIONS = ["vgm", "vgz", "png", "txt"];

// token -> FileSystemDirectoryHandle
const handles = new Map();

function extensionOf(name) {
  const dot = name.lastIndexOf(".");
  return dot < 0 ? "" : name.slice(dot + 1).toLowerCase();
}

function byLowercaseName(a, b) {
  const x = a.name.toLowerCase();
  const y = b.name.toLowerCase();
  return x < y ? -1 : x > y ? 1 : 0;
}

/** A token unique among the currently held folders (a re-pick starts fresh). */
function uniqueToken(name) {
  let token = name;
  let n = 2;
  while (handles.has(token)) token = `${name} (${n++})`;
  return token;
}

/** Scans a directory into `[{ name, bytes }]`, filtered + sorted like native. */
async function scanDir(handle) {
  const files = [];
  for await (const entry of handle.values()) {
    if (entry.kind !== "file") continue;
    if (!PACK_EXTENSIONS.includes(extensionOf(entry.name))) continue;
    try {
      const file = await entry.getFile();
      const bytes = new Uint8Array(await file.arrayBuffer());
      files.push({ name: entry.name, bytes });
    } catch (error) {
      // One unreadable entry is skipped, not fatal -- as the native scan does.
      console.warn(`pack_fs: skipping unreadable ${entry.name}: ${error}`);
    }
  }
  files.sort(byLowercaseName);
  return files;
}

/** Obtains a directory handle: the e2e override if present, else the picker. */
async function chooseDirectory(purpose) {
  const override = globalThis.__vgms_pick_dir;
  if (typeof override === "function") return override(purpose);
  if (typeof self.showDirectoryPicker !== "function") {
    throw new Error(
      "This browser has no folder picker. Open a .zip pack instead.",
    );
  }
  return self.showDirectoryPicker({ mode: "readwrite", id: "vgms-pack" });
}

/** Picks a pack folder, scans it, and returns `{ token, name, files }` (or null
 *  if the user dismissed the picker). */
export async function pickPackFolder() {
  let handle;
  try {
    handle = await chooseDirectory("pack");
  } catch (error) {
    if (error && error.name === "AbortError") return null;
    throw error;
  }
  if (!handle) return null;
  const token = uniqueToken(handle.name);
  handles.set(token, handle);
  return { token, name: handle.name, files: await scanDir(handle) };
}

/** Picks an output folder for the split flow: `{ token, name }` or null. */
export async function pickOutputFolder() {
  let handle;
  try {
    handle = await chooseDirectory("output");
  } catch (error) {
    if (error && error.name === "AbortError") return null;
    throw error;
  }
  if (!handle) return null;
  const token = uniqueToken(handle.name);
  handles.set(token, handle);
  return { token, name: handle.name };
}

/** Re-scans a held folder (the rescan / tab-return path). */
export async function rescanPackFolder(token) {
  const handle = handles.get(token);
  if (!handle) throw new Error(`unknown pack folder: ${token}`);
  return { token, name: handle.name, files: await scanDir(handle) };
}

/** Writes (creating or overwriting) a file in a held folder. */
export async function writePackFile(token, name, bytes) {
  const handle = handles.get(token);
  if (!handle) throw new Error(`unknown pack folder: ${token}`);
  const fileHandle = await handle.getFileHandle(name, { create: true });
  const writable = await fileHandle.createWritable();
  await writable.write(bytes);
  await writable.close();
}

/** Deletes a file from a held folder. */
export async function deletePackFile(token, name) {
  const handle = handles.get(token);
  if (!handle) throw new Error(`unknown pack folder: ${token}`);
  await handle.removeEntry(name);
}

async function exists(handle, name) {
  try {
    await handle.getFileHandle(name);
    return true;
  } catch {
    return false;
  }
}

async function copyThenDelete(handle, fromName, toName) {
  const src = await handle.getFileHandle(fromName);
  const bytes = new Uint8Array(await (await src.getFile()).arrayBuffer());
  const dst = await handle.getFileHandle(toName, { create: true });
  const writable = await dst.createWritable();
  await writable.write(bytes);
  await writable.close();
  await handle.removeEntry(fromName);
}

/** A throwaway name in the same folder, for the case-only two-step. */
async function tempName(handle, base) {
  let n = 0;
  let candidate;
  do {
    candidate = `${base}.rename-tmp${n ? n : ""}`;
    n += 1;
  } while (await exists(handle, candidate));
  return candidate;
}

/** Renames a file, mirroring the native decision tree exactly. */
export async function renamePackFile(token, fromName, toName) {
  const handle = handles.get(token);
  if (!handle) throw new Error(`unknown pack folder: ${token}`);
  if (fromName === toName) return;

  const caseOnly = fromName.toLowerCase() === toName.toLowerCase();
  if (!caseOnly && (await exists(handle, toName))) {
    // Fail rather than overwrite -- the load-bearing half of the contract.
    throw new Error(`${toName} already exists`);
  }

  const src = await handle.getFileHandle(fromName);
  if (typeof src.move === "function") {
    if (caseOnly) {
      // A direct move to a case variant is a no-op on a case-insensitive store,
      // so bounce through a distinct temp name first.
      const temp = await tempName(handle, fromName);
      await src.move(temp);
      const moved = await handle.getFileHandle(temp);
      await moved.move(toName);
    } else {
      await src.move(toName);
    }
    return;
  }

  // No move(): copy + delete (through a temp for the case-only variant).
  if (caseOnly) {
    const temp = await tempName(handle, fromName);
    await copyThenDelete(handle, fromName, temp);
    await copyThenDelete(handle, temp, toName);
  } else {
    await copyThenDelete(handle, fromName, toName);
  }
}
