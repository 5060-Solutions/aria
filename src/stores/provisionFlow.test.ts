import { describe, expect, it, vi } from "vitest";
import { createProvisionFlow, previewMode, provisionedAccount } from "./provisionFlow";
import type { ProvisionApi, ProvisionedLine, ProvisionPreview } from "./provisionFlow";
import type { SipAccount } from "../types/sip";

const link = {
  redeemUrl: "https://api.ariaroute.com/provision/softphone/tok",
  serverHost: "api.ariaroute.com",
};

const preview: ProvisionPreview = {
  tenantName: "Regular Test",
  extension: "1001",
  displayName: "Front Desk",
  domain: "regular-test.ariaroute.com",
  expiresAt: null,
  serverHost: "api.ariaroute.com",
};

const line: ProvisionedLine = {
  accountId: "acc-1",
  mode: "added",
  provisionedFrom: "api.ariaroute.com",
  tenantName: "Regular Test",
  displayName: "Front Desk",
  username: "1001",
  authUsername: null,
  domain: "regular-test.ariaroute.com",
  outboundProxy: "sip.ariaroute.com:5061",
  tlsServerName: "sip.ariaroute.com",
  port: 5061,
  transport: "tls",
  voicemail: "*97",
};

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function mockApi(overrides: Partial<ProvisionApi> = {}) {
  return {
    preview: vi.fn<ProvisionApi["preview"]>(async () => preview),
    redeem: vi.fn<ProvisionApi["redeem"]>(async () => line),
    activate: vi.fn<ProvisionApi["activate"]>(async () => null),
    ...overrides,
  };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("provision flow", () => {
  it("shows the preview, and redeems only on confirm", async () => {
    const api = mockApi();
    const flow = createProvisionFlow(api);
    expect(flow.getState().open(link)).toBe(true);
    expect(flow.getState().state.phase).toBe("loading");
    await flush();
    expect(flow.getState().state).toEqual({ phase: "preview", link, preview });
    expect(api.preview).toHaveBeenCalledWith(link.redeemUrl);
    expect(api.redeem).not.toHaveBeenCalled();

    await flow.getState().confirm();
    expect(api.redeem).toHaveBeenCalledTimes(1);
    // The previewed line goes with the redeem, so the backend can refuse a different one.
    expect(api.redeem).toHaveBeenCalledWith(link.redeemUrl, preview);
    expect(api.activate).toHaveBeenCalledWith(line);
    expect(flow.getState().state).toEqual({ phase: "done", line, error: null });
  });

  it("Cancel never redeems", async () => {
    const api = mockApi();
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    await flush();
    flow.getState().cancel();
    expect(flow.getState().state.phase).toBe("idle");
    // A stray confirm after cancel (a double-click, a late keypress) does nothing.
    await flow.getState().confirm();
    expect(api.redeem).not.toHaveBeenCalled();
    expect(api.activate).not.toHaveBeenCalled();
  });

  it("cancel while the preview loads stops a late reply reopening the dialog", async () => {
    const pending = deferred<ProvisionPreview>();
    const api = mockApi({ preview: vi.fn(() => pending.promise) });
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    flow.getState().cancel();
    pending.resolve(preview);
    await flush();
    expect(flow.getState().state.phase).toBe("idle");
    await flow.getState().confirm();
    expect(api.redeem).not.toHaveBeenCalled();
  });

  it("confirm before the preview has loaded does nothing", async () => {
    const pending = deferred<ProvisionPreview>();
    const api = mockApi({ preview: vi.fn(() => pending.promise) });
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    await flow.getState().confirm();
    expect(api.redeem).not.toHaveBeenCalled();
  });

  it("a double click redeems once", async () => {
    const api = mockApi();
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    await flush();
    await Promise.all([flow.getState().confirm(), flow.getState().confirm()]);
    expect(api.redeem).toHaveBeenCalledTimes(1);
  });

  it("a second link while one is open is ignored", async () => {
    const api = mockApi();
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    await flush();
    const other = { redeemUrl: "https://evil.example/provision/softphone/x", serverHost: "evil.example" };
    expect(flow.getState().open(other)).toBe(false);
    expect(flow.getState().state).toEqual({ phase: "preview", link, preview });
    expect(api.preview).toHaveBeenCalledTimes(1);
  });

  it("cannot be cancelled mid-redeem", async () => {
    const pending = deferred<ProvisionedLine>();
    const api = mockApi({ redeem: vi.fn(() => pending.promise) });
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    await flush();
    const done = flow.getState().confirm();
    flow.getState().cancel();
    expect(flow.getState().state.phase).toBe("redeeming");
    pending.resolve(line);
    await done;
    expect(flow.getState().state.phase).toBe("done");
  });

  it("reports a preview failure, then can open again", async () => {
    const api = mockApi({
      preview: vi.fn(async () => {
        throw "This setup link has expired or was already used. Ask for a new one.";
      }),
    });
    const clock = { t: 0 };
    const flow = createProvisionFlow(api, { now: () => clock.t });
    flow.getState().open(link);
    await flush();
    expect(flow.getState().state).toEqual({
      phase: "failed",
      message: "This setup link has expired or was already used. Ask for a new one.",
    });
    flow.getState().cancel();
    clock.t += 3000;
    expect(flow.getState().open(link)).toBe(true);
  });

  it("reports a redeem failure without activating", async () => {
    const api = mockApi({ redeem: vi.fn(async () => Promise.reject(new Error("HTTP 500"))) });
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    await flush();
    await flow.getState().confirm();
    expect(flow.getState().state).toEqual({ phase: "failed", message: "HTTP 500" });
    expect(api.activate).not.toHaveBeenCalled();
  });

  it("keeps the line and reports a registration error", async () => {
    const api = mockApi({ activate: vi.fn(async () => "403 Forbidden") });
    const flow = createProvisionFlow(api);
    flow.getState().open(link);
    await flush();
    await flow.getState().confirm();
    expect(flow.getState().state).toEqual({ phase: "done", line, error: "403 Forbidden" });
  });

  it("ignores links for a few seconds after the dialog closes", async () => {
    const clock = { t: 1000 };
    const api = mockApi();
    const flow = createProvisionFlow(api, { now: () => clock.t, cooldownMs: 3000 });
    flow.getState().open(link);
    await flush();
    flow.getState().cancel();
    clock.t += 2999;
    expect(flow.getState().open(link)).toBe(false);
    expect(flow.getState().state.phase).toBe("idle");
    clock.t += 1;
    expect(flow.getState().open(link)).toBe(true);
    expect(api.preview).toHaveBeenCalledTimes(2);
  });

  it("only one preview is in flight", () => {
    const pending = deferred<ProvisionPreview>();
    const api = mockApi({ preview: vi.fn(() => pending.promise) });
    const flow = createProvisionFlow(api);
    expect(flow.getState().open(link)).toBe(true);
    expect(flow.getState().open(link)).toBe(false);
    expect(api.preview).toHaveBeenCalledTimes(1);
  });
});

const account = (over: Partial<SipAccount>): SipAccount => ({
  id: "x",
  displayName: "My desk phone",
  username: "1001",
  domain: "regular-test.ariaroute.com",
  password: "",
  transport: "udp",
  port: 5060,
  enabled: true,
  ...over,
});

describe("previewMode", () => {
  it("adds when no account has the identity", () => {
    expect(previewMode(preview, [account({ username: "1002" })])).toBe("added");
  });

  it("updates only an account the same host set up", () => {
    expect(
      previewMode(preview, [account({ domain: "Regular-Test.AriaRoute.com", provisionedFrom: "API.ariaroute.com" })]),
    ).toBe("updated");
  });

  it("adds a separate line beside a hand-made or other-host account", () => {
    expect(previewMode(preview, [account({})])).toBe("separate");
    expect(previewMode(preview, [account({ provisionedFrom: "evil.example" })])).toBe("separate");
  });
});

describe("provisionedAccount", () => {
  it("builds a new account from the line", () => {
    const a = provisionedAccount(line, undefined);
    expect(a).toMatchObject({
      id: "acc-1",
      displayName: "Front Desk",
      username: "1001",
      domain: "regular-test.ariaroute.com",
      password: "",
      transport: "tls",
      port: 5061,
      outboundProxy: "sip.ariaroute.com:5061",
      tlsServerName: "sip.ariaroute.com",
      provisionedFrom: "api.ariaroute.com",
      voicemailNumber: "*97",
      enabled: true,
    });
    expect(a.registrar).toBeUndefined();
  });

  it("an update replaces server settings and keeps the user's own", () => {
    const existing = account({
      id: "acc-1",
      displayName: "My desk phone",
      enabled: false,
      registrar: "old.example",
      outboundProxy: "old.example:5061",
      authRealm: "realm.example",
      srtpMode: "sdes",
      codecs: [{ codec: "pcmu", enabled: true, priority: 1 }],
      autoRecord: true,
      autoCountryCode: true,
      stripPrefix: "9",
      prependPrefix: "1",
      provisionedFrom: "api.ariaroute.com",
    });
    const a = provisionedAccount({ ...line, mode: "updated", displayName: "Server Name" }, existing);
    expect(a).toMatchObject({
      id: "acc-1",
      displayName: "My desk phone",
      enabled: false,
      authRealm: "realm.example",
      srtpMode: "sdes",
      codecs: [{ codec: "pcmu", enabled: true, priority: 1 }],
      autoRecord: true,
      autoCountryCode: true,
      stripPrefix: "9",
      prependPrefix: "1",
      transport: "tls",
      port: 5061,
      outboundProxy: "sip.ariaroute.com:5061",
      tlsServerName: "sip.ariaroute.com",
      password: "",
    });
    expect(a.registrar).toBeUndefined();
  });
});
