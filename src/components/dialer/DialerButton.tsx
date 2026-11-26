import { useRef } from "react";
import { ButtonBase, Box, alpha, useTheme } from "@mui/material";
import { motion } from "framer-motion";
import { playDtmfTone } from "../../audio/dtmf";

interface DialerButtonProps {
  digit: string;
  letters?: string;
  onPress: (digit: string) => void;
  /** Held for half a second instead of a tap: long-press 0 types "+". */
  onLongPress?: () => void;
}

const LONG_PRESS_MS = 500;

export function DialerButton({ digit, letters, onPress, onLongPress }: DialerButtonProps) {
  const theme = useTheme();
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const held = useRef(false);

  const cancelHold = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
  };

  const startHold = () => {
    held.current = false;
    if (!onLongPress) return;
    cancelHold();
    timer.current = setTimeout(() => {
      timer.current = null;
      held.current = true;
      onLongPress();
    }, LONG_PRESS_MS);
  };

  const handlePress = () => {
    // The hold already typed its character; the click that ends it must not add the digit.
    if (held.current) {
      held.current = false;
      return;
    }
    playDtmfTone(digit);
    onPress(digit);
  };

  return (
    <motion.div whileTap={{ scale: 0.9 }} transition={{ duration: 0.08 }}>
      <ButtonBase
        onClick={handlePress}
        onPointerDown={startHold}
        onPointerUp={cancelHold}
        onPointerLeave={cancelHold}
        onPointerCancel={cancelHold}
        sx={{
          width: "var(--dial-key, 76px)",
          height: "var(--dial-key, 76px)",
          borderRadius: "50%",
          bgcolor: alpha(theme.palette.text.primary, 0.07),
          color: "text.primary",
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          transition: "background-color 0.12s ease",
          "&:hover": {
            bgcolor: alpha(theme.palette.text.primary, 0.12),
          },
          "&:active": {
            bgcolor: alpha(theme.palette.primary.main, 0.12),
          },
        }}
      >
        <Box
          sx={{
            fontSize: "1.7rem",
            fontWeight: 300,
            lineHeight: 1.1,
            fontFamily: '"Google Sans", sans-serif',
          }}
        >
          {digit}
        </Box>
        {/* Every key keeps the letters row, empty on 1, * and #, so the digit
            sits at the same height on every key. */}
        <Box
          aria-hidden={!letters}
          sx={{
            fontSize: "0.55rem",
            fontWeight: 600,
            letterSpacing: "0.18em",
            opacity: 0.55,
            lineHeight: 1,
            mt: 0.2,
            minHeight: "0.55rem",
          }}
        >
          {letters ?? "\u00A0"}
        </Box>
      </ButtonBase>
    </motion.div>
  );
}
