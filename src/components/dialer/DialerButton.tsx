import { ButtonBase, Box, alpha, useTheme } from "@mui/material";
import { motion } from "framer-motion";
import { playDtmfTone } from "../../audio/dtmf";

interface DialerButtonProps {
  digit: string;
  letters?: string;
  onPress: (digit: string) => void;
}

export function DialerButton({ digit, letters, onPress }: DialerButtonProps) {
  const theme = useTheme();

  const handlePress = () => {
    playDtmfTone(digit);
    onPress(digit);
  };

  return (
    <motion.div whileTap={{ scale: 0.9 }} transition={{ duration: 0.08 }}>
      <ButtonBase
        onClick={handlePress}
        sx={{
          width: 76,
          height: 76,
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
        {letters && (
          <Box
            sx={{
              fontSize: "0.55rem",
              fontWeight: 600,
              letterSpacing: "0.18em",
              opacity: 0.55,
              lineHeight: 1,
              mt: 0.2,
            }}
          >
            {letters}
          </Box>
        )}
      </ButtonBase>
    </motion.div>
  );
}
