/** The colour and label of each registration state, shared by the status bar and the connection panel. */
export const stateColors: Record<string, string> = {
  registered: "#4caf50",
  registering: "#ff9800",
  reconnecting: "#ff9800",
  unregistered: "#9e9e9e",
  error: "#f44336",
};

export const stateLabelsKeys: Record<string, string> = {
  registered: "status.connected",
  registering: "status.connecting",
  reconnecting: "status.reconnecting",
  unregistered: "status.notConnected",
  error: "status.connectionError",
};
