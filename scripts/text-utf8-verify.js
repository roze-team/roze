"use strict";

const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "..");
const textExtensions = new Set([
  ".css",
  ".csv",
  ".ent",
  ".go",
  ".html",
  ".js",
  ".json",
  ".md",
  ".proto",
  ".ps1",
  ".rs",
  ".search",
  ".sh",
  ".sql",
  ".svg",
  ".toml",
  ".ts",
  ".tsx",
  ".txt",
  ".yaml",
  ".yml",
]);

function verifyTextBuffer(buffer, label) {
  let text;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(buffer);
  } catch (error) {
    throw new Error(`${label}: invalid UTF-8 (${error.message})`);
  }
  if (text.includes("\uFFFD")) {
    throw new Error(`${label}: contains Unicode replacement character U+FFFD`);
  }
}

const excludedDirectories = new Set([
  ".git",
  ".idea",
  ".tmp",
  ".vscode",
  "node_modules",
  "target",
  "vendor",
]);

function repositoryTextFiles(repositoryRoot = root) {
  const files = [];
  const visit = (directory) => {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      if (entry.isSymbolicLink()) continue;
      if (entry.isDirectory()) {
        if (
          excludedDirectories.has(entry.name) ||
          entry.name.startsWith("target-") ||
          entry.name.startsWith(".tmp-")
        ) {
          continue;
        }
        visit(path.join(directory, entry.name));
        continue;
      }
      if (
        entry.isFile() &&
        textExtensions.has(path.extname(entry.name).toLowerCase())
      ) {
        files.push(path.relative(repositoryRoot, path.join(directory, entry.name)));
      }
    }
  };
  visit(repositoryRoot);
  return files.sort();
}

function verifyRepository(repositoryRoot = root) {
  const files = repositoryTextFiles(repositoryRoot);
  for (const relative of files) {
    const absolute = path.join(repositoryRoot, relative);
    verifyTextBuffer(fs.readFileSync(absolute), relative);
  }
  return files.length;
}

if (require.main === module) {
  try {
    const count = verifyRepository();
    console.log(`repository text UTF-8 valid: ${count} files`);
  } catch (error) {
    console.error(`tracked text UTF-8 invalid: ${error.message}`);
    process.exit(1);
  }
}

module.exports = { repositoryTextFiles, verifyRepository, verifyTextBuffer };
