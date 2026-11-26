import { Box, Typography } from "@mui/material";
import { KEYPAD_KEYS } from "../../utils/inCallKeypad";
import { DialerButton } from "../dialer/DialerButton";

interface InCallKeypadProps {
  /** What has been sent on this call, shown above the keys. */
  digits: string;
  onDigit: (digit: string) => void;
}

/**
 * The DTMF keypad, in the call screen's own column between the caller and the
 * controls (it used to float over the call info). It takes the height that is
 * left: the keys are sized from the window, never below 30px, and the grid
 * clips before it pushes the caller or the controls out.
 */
export function InCallKeypad({ digits, onDigit }: InCallKeypadProps) {
  return (
    <Box
      data-testid="in-call-keypad"
      sx={{
        // Window height less the status bar, caller block, digits line and
        // controls, shared by four rows of keys. The call screen's own
        // spacing tightens below 680px of height (see callScreenLayout), so
        // the amount taken off does too.
        "--dial-key": "clamp(30px, calc((100vh - 520px) / 4), 60px)",
        "@media (max-height: 680px)": { "--dial-key": "clamp(30px, calc((100vh - 460px) / 4), 56px)" },
        width: "100%",
        flex: "1 1 auto",
        minHeight: 0,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: 0.5,
        overflow: "hidden",
      }}
    >
      <Typography
        data-testid="in-call-digits"
        variant="body1"
        aria-live="polite"
        sx={{
          flexShrink: 0,
          minHeight: "1.5em",
          maxWidth: "100%",
          overflow: "hidden",
          whiteSpace: "nowrap",
          direction: "rtl",
          textOverflow: "clip",
          fontVariantNumeric: "tabular-nums",
          letterSpacing: "0.12em",
          color: "text.primary",
        }}
      >
        {/* rtl shows the latest digits when the string outgrows the line; the
            bidi mark keeps `*` and `#` from moving to the other end. */}
        <bdi dir="ltr">{digits}</bdi>
      </Typography>
      <Box
        sx={{
          minHeight: 0,
          display: "grid",
          gridTemplateColumns: "repeat(3, var(--dial-key))",
          gap: "6px",
          justifyContent: "center",
          overflow: "hidden",
        }}
      >
        {KEYPAD_KEYS.map(({ digit, letters }) => (
          <DialerButton
            key={digit}
            digit={digit}
            letters={letters || undefined}
            onPress={onDigit}
          />
        ))}
      </Box>
    </Box>
  );
}
