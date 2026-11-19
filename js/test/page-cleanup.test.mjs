import assert from "node:assert/strict";
import test from "node:test";
import { closeExtraPages, maximizeWindow } from "../src/automation.mjs";

function contextWith(pages, name, fail = false) {
  return {
    name,
    async close() {
      if (fail) {
        throw new Error(`${name} already closed`);
      }
      const index = pages.indexOf(this);
      if (index >= 0) {
        pages.splice(index, 1);
      }
    },
    context() {
      return { pages: () => pages };
    },
  };
}

test("extra restored tabs are closed and the working page is kept", async () => {
  const pages = [];
  const one = contextWith(pages, "one");
  const working = contextWith(pages, "working");
  const two = contextWith(pages, "two");
  pages.push(one, working, two);
  assert.equal(await closeExtraPages(working), 2);
  assert.deepEqual(pages, [working]);
});

test("a tab that closes during cleanup does not stop the others", async () => {
  const pages = [];
  const failing = contextWith(pages, "failing", true);
  const working = contextWith(pages, "working");
  const closing = contextWith(pages, "closing");
  pages.push(failing, working, closing);
  assert.equal(await closeExtraPages(working), 1);
  assert.ok(pages.includes(working));
  assert.ok(!pages.includes(closing));
});

test("a visible window is maximized through CDP", async () => {
  const calls = [];
  const page = {
    context: () => ({
      newCDPSession: async () => ({
        send: async (method, params) => {
          calls.push([method, params]);
          return method === "Browser.getWindowForTarget"
            ? { windowId: 7 }
            : undefined;
        },
        detach: async () => calls.push(["detach"]),
      }),
    }),
  };
  assert.equal(await maximizeWindow(page), true);
  assert.deepEqual(calls[0], ["Browser.getWindowForTarget", undefined]);
  assert.deepEqual(calls[1], [
    "Browser.setWindowBounds",
    { windowId: 7, bounds: { windowState: "maximized" } },
  ]);
  assert.deepEqual(calls[2], ["detach"]);
});

test("a page without CDP is left alone", async () => {
  assert.equal(await maximizeWindow({ context: () => ({}) }), false);
});

test("a working-page wrapper with the same URL is kept", async () => {
  const pages = [];
  const actual = contextWith(pages, "actual");
  actual.url = () => "https://www.npmjs.com/login";
  const extra = contextWith(pages, "extra");
  extra.url = () => "https://example.test/";
  pages.push(actual, extra);
  const wrapper = {
    url: () => "https://www.npmjs.com/login",
    context: () => ({ pages: () => pages }),
  };
  assert.equal(await closeExtraPages(wrapper), 1);
  assert.deepEqual(pages, [actual]);
});
