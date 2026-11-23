import { describe, expect, it, vi } from "vitest";
import { backendDeviceName, pushAudioDevices } from "./audioDevices";

describe("backendDeviceName", () => {
  it("passes a device name through", () => {
    expect(backendDeviceName("USB Headset")).toBe("USB Headset");
  });

  it("sends the system default as null, however it was stored", () => {
    expect(backendDeviceName("default")).toBeNull();
    expect(backendDeviceName(null)).toBeNull();
    expect(backendDeviceName(undefined)).toBeNull();
    expect(backendDeviceName("")).toBeNull();
  });
});

describe("pushAudioDevices", () => {
  it("sends the saved microphone and speaker to the backend", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);
    await pushAudioDevices(invoke, "USB Headset", "default");
    expect(invoke).toHaveBeenCalledWith("set_audio_devices", {
      inputDevice: "USB Headset",
      outputDevice: null,
    });
  });

  it("sends nothing but defaults when nothing was saved", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);
    await pushAudioDevices(invoke, null, null);
    expect(invoke).toHaveBeenCalledWith("set_audio_devices", { inputDevice: null, outputDevice: null });
  });
});
