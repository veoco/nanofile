import { test, expect } from "@playwright/test";

// The error pages. Every wire endpoint keeps its JSON (covered by the Rust
// tests); these check what a browser actually lands on.
//
// The config's default storage state is a logged-in session, which is the
// interesting case for the in-app pages and the wrong one for the public
// routes — hence the signed-out block below.

test.describe("signed in", () => {
  test("an unknown URL renders a page inside the app", async ({ page }) => {
    await page.goto("/no/such/page/");

    await expect(page.locator("h1")).toHaveText("Page not found");
    // The app chrome is still there, so the reader is not stranded.
    await expect(page.locator(".js-left-panel")).toBeVisible();
    await expect(page.getByRole("link", { name: "Back to libraries" })).toBeVisible();

    await page.getByRole("link", { name: "Back to libraries" }).click();
    await expect(page).toHaveURL(/\/libraries\/$/);
  });

  test("a library that does not exist renders a page, not JSON", async ({
    page,
  }) => {
    await page.goto("/libraries/00000000-0000-0000-0000-000000000000/files/");

    await expect(page.locator("h1")).toHaveText("Page not found");
    await expect(page.locator("body")).not.toContainText("error_msg");
  });
});

test.describe("signed out", () => {
  test.use({ storageState: { cookies: [], origins: [] } });

  test("an unknown URL offers the one thing a guest can do", async ({ page }) => {
    await page.goto("/no/such/page/");

    await expect(page.locator("h1")).toHaveText("Page not found");
    const signIn = page.getByRole("link", { name: "Sign in" });
    await expect(signIn).toBeVisible();
    // A guest cannot go "back to libraries", so they are not offered it.
    await expect(page.getByRole("link", { name: "Back to libraries" })).toHaveCount(0);

    await signIn.click();
    await expect(page).toHaveURL(/\/accounts\/login\//);
  });

  test("a dead share link explains itself without a status code", async ({ page }) => {
    const response = await page.goto("/d/no-such-token-at-all/");
    expect(response?.status()).toBe(404);

    await expect(page.locator("h1")).toHaveText("Link unavailable");
    // No "404" in the reader's face: the code is the server's story.
    await expect(page.locator("body")).not.toContainText("404");
    await expect(page.getByRole("link", { name: "Go to Nanofile" })).toBeVisible();
  });

  test("a dead upload link explains itself the same way", async ({ page }) => {
    await page.goto("/u/no-such-token-at-all/");
    await expect(page.locator("h1")).toHaveText("Link unavailable");
  });

  test("a missing static asset is still plain text", async ({ request }) => {
    const response = await request.get("/static/css/no-such-file.css");
    expect(response.status()).toBe(404);
    expect(await response.text()).toBe("404 Not Found");
  });
});
