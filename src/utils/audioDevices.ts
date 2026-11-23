/**
 * The audio device choice as the backend takes it: a device name, or `null`
 * for the system default. Settings stores the default as the string
 * "default", which is not a device name.
 */
export function backendDeviceName(name: string | null | undefined): string | null {
  return name && name !== "default" ? name : null;
}

type Invoke = (cmd: string, args: Record<string, unknown>) => Promise<unknown>;

/**
 * Tell the backend which microphone and speaker calls should use.
 *
 * The backend keeps the choice in memory only, so the saved one has to be
 * pushed on every launch. Without that, a USB headset picked in Settings
 * showed as selected after a restart while calls went to the system default.
 */
export function pushAudioDevices(
  invoke: Invoke,
  input: string | null | undefined,
  output: string | null | undefined,
): Promise<unknown> {
  return invoke("set_audio_devices", {
    inputDevice: backendDeviceName(input),
    outputDevice: backendDeviceName(output),
  });
}
