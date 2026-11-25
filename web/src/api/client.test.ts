import { describe, expect, it, vi } from "vitest";
import { listRecipes } from "./client";

describe("listRecipes", () => {
  it("throws on an error status", async () => {
    vi.stubGlobal("fetch", async () => new Response("", { status: 500 }));
    await expect(listRecipes()).rejects.toThrow("recipes failed: 500");
  });

  it("returns the recipes", async () => {
    vi.stubGlobal("fetch", async () => new Response("[]"));
    await expect(listRecipes()).resolves.toEqual([]);
  });
});
