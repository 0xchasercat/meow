import fs from "node:fs";

const api = fs.promises;

export const access = api.access;
export const mkdir = api.mkdir;
export const readFile = api.readFile;
export const readdir = api.readdir;
export const rm = api.rm;
export const stat = api.stat;
export const writeFile = api.writeFile;

export default api;
