/** The colour a registration state is shown in, shared by the cards and the tabs. */
export function registrationColor(state: string): string {
  switch (state) {
    case "registered":
      return "#4caf50";
    case "registering":
    case "reconnecting":
      return "#ff9800";
    case "error":
      return "#f44336";
    default:
      return "#9e9e9e";
  }
}
