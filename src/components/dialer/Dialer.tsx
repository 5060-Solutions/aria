import { useState, useMemo, useCallback, useEffect } from "react";
import { Box, IconButton, alpha, useTheme, Typography, Autocomplete, TextField, Paper, Popper } from "@mui/material";
import CallIcon from "@mui/icons-material/Call";
import BackspaceOutlinedIcon from "@mui/icons-material/BackspaceOutlined";
import { motion, AnimatePresence } from "framer-motion";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { sipMakeCall } from "../../hooks/useSip";
import { DialerButton } from "./DialerButton";
import { parsePhoneNumber, AsYouType, type CountryCode, getCountries, getCountryCallingCode } from "libphonenumber-js";

const keys = [
  { digit: "1", letters: "" },
  { digit: "2", letters: "ABC" },
  { digit: "3", letters: "DEF" },
  { digit: "4", letters: "GHI" },
  { digit: "5", letters: "JKL" },
  { digit: "6", letters: "MNO" },
  { digit: "7", letters: "PQRS" },
  { digit: "8", letters: "TUV" },
  { digit: "9", letters: "WXYZ" },
  { digit: "*", letters: "" },
  { digit: "0", letters: "+" },
  { digit: "#", letters: "" },
];

const popularCountries: CountryCode[] = ["US", "GB", "CA", "AU", "DE", "FR", "ES", "IT", "JP", "CN", "IN", "BR", "MX"];

function getFlagEmoji(countryCode: string): string {
  const codePoints = countryCode
    .toUpperCase()
    .split("")
    .map((char) => 127397 + char.charCodeAt(0));
  return String.fromCodePoint(...codePoints);
}

export function Dialer() {
  const { t, i18n } = useTranslation();
  const dialInput = useAppStore((s) => s.dialInput);
  const appendDigit = useAppStore((s) => s.appendDigit);
  const clearDialInput = useAppStore((s) => s.clearDialInput);
  const setDialInput = useAppStore((s) => s.setDialInput);
  const setActiveCall = useAppStore((s) => s.setActiveCall);
  const defaultCountry = useAppStore((s) => s.defaultCountry) as CountryCode;
  const setDefaultCountry = useAppStore((s) => s.setDefaultCountry);
  const theme = useTheme();

  const accounts = useAppStore((s) => s.accounts);
  const activeAccountId = useAppStore((s) => s.activeAccountId);
  const activeAccount = accounts.find((a) => a.id === activeAccountId);

  const [countryPickerOpen, setCountryPickerOpen] = useState(false);
  
  const countryDisplayNames = useMemo(() => {
    return new Intl.DisplayNames([i18n.language], { type: "region" });
  }, [i18n.language]);
  
  const getCountryName = useCallback((code: string) => {
    try {
      return countryDisplayNames.of(code) || code;
    } catch {
      return code;
    }
  }, [countryDisplayNames]);

  const formattedNumber = useMemo(() => {
    if (!dialInput) return "";
    
    // Get just the digits to check length
    const digits = dialInput.replace(/\D/g, "");
    
    // Don't format short numbers (extensions, test numbers like 9998)
    // unless they start with + (international format)
    const isInternational = dialInput.startsWith("+");
    const isShortNumber = digits.length <= 6 && !isInternational;
    
    if (isShortNumber) {
      return dialInput; // Return as-is for extensions
    }
    
    try {
      const formatter = new AsYouType(defaultCountry);
      return formatter.input(dialInput);
    } catch {
      return dialInput;
    }
  }, [dialInput, defaultCountry]);

  const detectedCountry = useMemo(() => {
    if (!dialInput) return null;
    
    // For numbers starting with +, try to detect country from calling code
    if (dialInput.startsWith("+")) {
      const digits = dialInput.slice(1).replace(/\D/g, "");
      if (digits.length >= 1) {
        // Check country codes (1-3 digits) - try longest match first
        for (const len of [3, 2, 1]) {
          if (digits.length >= len) {
            const codeToCheck = digits.slice(0, len);
            // Find country with this calling code
            for (const country of getCountries()) {
              try {
                if (getCountryCallingCode(country) === codeToCheck) {
                  return country;
                }
              } catch {
                continue;
              }
            }
          }
        }
      }
    }
    
    // Fall back to parsing complete numbers
    if (dialInput.length >= 3) {
      try {
        const parsed = parsePhoneNumber(dialInput, defaultCountry);
        return parsed?.country || null;
      } catch {
        return null;
      }
    }
    
    return null;
  }, [dialInput, defaultCountry]);

  const displayCountry = detectedCountry || defaultCountry;
  
  // Determine if we should show the country selector (hide for extensions)
  const digits = dialInput.replace(/\D/g, "");
  const isInternational = dialInput.startsWith("+");
  const showCountrySelector = !dialInput || isInternational || digits.length > 6;

  const handleCall = useCallback(async () => {
    if (!dialInput.trim() || !activeAccountId || !activeAccount) return;
    
    let numberToCall = dialInput.replace(/[\s\-()]/g, "");
    
    try {
      const parsed = parsePhoneNumber(dialInput, defaultCountry);
      if (parsed?.isValid()) {
        numberToCall = parsed.number.replace("+", "");
      }
    } catch {
      // Use raw input
    }
    
    const domain = activeAccount.domain;
    const uri = dialInput.includes("@")
      ? `sip:${dialInput}`
      : `sip:${numberToCall}@${domain}`;

    setActiveCall({
      id: crypto.randomUUID(),
      accountId: activeAccountId,
      remoteUri: uri,
      remoteName: formattedNumber || dialInput,
      state: "dialing",
      direction: "outbound",
      startTime: Date.now(),
      muted: false,
      held: false,
      recording: false,
    });

    try {
      const callId = await sipMakeCall(uri);
      setActiveCall({
        id: callId,
        accountId: activeAccountId,
        remoteUri: uri,
        remoteName: formattedNumber || dialInput,
        state: "dialing",
        direction: "outbound",
        startTime: Date.now(),
        muted: false,
        held: false,
        recording: false,
      });
    } catch {
      setActiveCall(null);
    }
  }, [dialInput, activeAccountId, activeAccount, defaultCountry, formattedNumber, setActiveCall]);

  const handleBackspace = useCallback(() => {
    setDialInput(dialInput.slice(0, -1));
  }, [dialInput, setDialInput]);

  // Keyboard input support
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // Ignore if user is typing in an input field
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) {
        return;
      }

      const key = e.key;

      // Digits 0-9
      if (/^[0-9]$/.test(key)) {
        appendDigit(key);
        return;
      }

      // Special characters
      if (key === "*" || key === "#") {
        appendDigit(key);
        return;
      }

      // Plus sign (for international dialing)
      if (key === "+" || (e.shiftKey && key === "=")) {
        appendDigit("+");
        return;
      }

      // Backspace
      if (key === "Backspace") {
        e.preventDefault();
        handleBackspace();
        return;
      }

      // Delete/Clear (clear all)
      if (key === "Delete" || key === "Escape") {
        clearDialInput();
        return;
      }

      // Enter to call
      if (key === "Enter" && dialInput.trim()) {
        handleCall();
        return;
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [appendDigit, handleBackspace, clearDialInput, dialInput, handleCall]);

  const allCountries = useMemo(() => {
    const all = getCountries();
    const sorted = [...popularCountries];
    all.forEach((c) => {
      if (!sorted.includes(c)) sorted.push(c);
    });
    return sorted;
  }, []);

  return (
    <Box
      sx={{
        height: "100%",
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "flex-end",
        px: 3,
        pb: 3,
        pt: 2,
      }}
    >
      {/* Number display with country flag */}
      <Box
        sx={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          width: "100%",
          minHeight: 60,
        }}
      >
        {/* Country selector - hidden for extensions/short numbers */}
        {showCountrySelector && (
          countryPickerOpen ? (
            <Autocomplete
              open
              onClose={() => setCountryPickerOpen(false)}
              options={allCountries}
              value={defaultCountry}
              onChange={(_, newValue) => {
                if (newValue) setDefaultCountry(newValue);
                setCountryPickerOpen(false);
              }}
              disableClearable
              autoHighlight
              size="small"
              getOptionLabel={(option) => {
                const name = getCountryName(option);
                const code = getCountryCallingCode(option);
                return `${name} +${code}`;
              }}
              filterOptions={(options, { inputValue }) => {
                const search = inputValue.toLowerCase();
                return options.filter((option) => {
                  const name = getCountryName(option).toLowerCase();
                  const code = getCountryCallingCode(option);
                  return name.includes(search) || code.includes(search) || option.toLowerCase().includes(search);
                });
              }}
              renderOption={(props, option) => {
                const { key, ...rest } = props;
                return (
                  <Box
                    component="li"
                    key={key}
                    {...rest}
                    sx={{
                      display: "flex",
                      alignItems: "center",
                      gap: 1.5,
                      py: 1,
                    }}
                  >
                    <Typography sx={{ fontSize: "1.2rem" }}>
                      {getFlagEmoji(option)}
                    </Typography>
                    <Box sx={{ flex: 1 }}>
                      <Typography variant="body2" sx={{ fontWeight: option === defaultCountry ? 600 : 400 }}>
                        {getCountryName(option)}
                      </Typography>
                      <Typography variant="caption" color="text.secondary">
                        +{getCountryCallingCode(option)}
                      </Typography>
                    </Box>
                  </Box>
                );
              }}
              slots={{
                paper: (props) => (
                  <Paper {...props} sx={{ ...props.sx, borderRadius: "14px", mt: 0.5 }} />
                ),
                popper: (props) => (
                  <Popper {...props} placement="bottom-start" sx={{ width: 280, zIndex: 1300 }} />
                ),
              }}
              renderInput={(params) => (
                <TextField
                  {...params}
                  autoFocus
                  placeholder={t("dialer.searchCountry")}
                  slotProps={{
                    input: {
                      ...params.InputProps,
                      startAdornment: (
                        <Typography sx={{ fontSize: "1.2rem", ml: 0.5, mr: 0.5 }}>
                          {getFlagEmoji(defaultCountry)}
                        </Typography>
                      ),
                    },
                  }}
                  sx={{
                    width: 260,
                    "& .MuiOutlinedInput-root": {
                      borderRadius: "14px",
                    },
                  }}
                />
              )}
              sx={{ mb: 1 }}
            />
          ) : (
            <Box
              onClick={() => setCountryPickerOpen(true)}
              sx={{
                display: "flex",
                alignItems: "center",
                gap: 0.5,
                cursor: "pointer",
                px: 1.5,
                py: 0.5,
                borderRadius: "12px",
                mb: 1,
                transition: "all 0.15s ease",
                "&:hover": {
                  bgcolor: alpha(theme.palette.primary.main, 0.08),
                },
              }}
            >
              <Typography sx={{ fontSize: "1.2rem" }}>
                {getFlagEmoji(displayCountry)}
              </Typography>
              <Typography
                variant="caption"
                sx={{ color: "text.secondary", fontWeight: 500 }}
              >
                +{getCountryCallingCode(displayCountry)}
              </Typography>
            </Box>
          )
        )}

        <AnimatePresence mode="wait">
          <motion.div
            key={formattedNumber || "placeholder"}
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            transition={{ duration: 0.08 }}
            style={{ textAlign: "center", width: "100%" }}
          >
            <Box
              sx={{
                fontSize:
                  formattedNumber.length > 18
                    ? "1.1rem"
                    : formattedNumber.length > 14
                      ? "1.4rem"
                      : formattedNumber.length > 10
                        ? "1.8rem"
                        : "2.2rem",
                fontWeight: 200,
                fontFamily: '"Google Sans", sans-serif',
                color: dialInput ? "text.primary" : "text.secondary",
                letterSpacing: dialInput ? "0.06em" : "0",
                transition: "font-size 0.15s ease",
                px: 1,
                whiteSpace: "nowrap",
                overflow: "hidden",
                textOverflow: "ellipsis",
                maxWidth: "100%",
              }}
            >
              {formattedNumber || t("dialer.enterNumber")}
            </Box>
          </motion.div>
        </AnimatePresence>
      </Box>

      {/* Keypad grid */}
      <Box
        sx={{
          display: "grid",
          gridTemplateColumns: "repeat(3, 76px)",
          gap: 1.5,
          justifyContent: "center",
          mb: 3,
        }}
      >
        {keys.map(({ digit, letters }) => (
          <DialerButton
            key={digit}
            digit={digit}
            letters={letters || undefined}
            onPress={appendDigit}
          />
        ))}
      </Box>

      {/* Call / backspace row */}
      <Box
        sx={{
          display: "grid",
          gridTemplateColumns: "48px 1fr 48px",
          alignItems: "center",
          justifyItems: "center",
          width: 260,
        }}
      >
        <Box />

        {/* Call FAB */}
        <motion.div whileTap={{ scale: 0.88 }} whileHover={{ scale: 1.04 }}>
          <IconButton
            onClick={handleCall}
            disabled={!dialInput.trim()}
            sx={{
              width: 64,
              height: 64,
              borderRadius: "22px",
              bgcolor: "primary.main",
              color: "primary.contrastText",
              boxShadow: dialInput
                ? `0 6px 24px ${alpha(theme.palette.primary.main, 0.4)}`
                : "none",
              transition: "all 0.2s ease",
              "&:hover": { bgcolor: "primary.dark" },
              "&.Mui-disabled": {
                bgcolor: alpha(theme.palette.primary.main, 0.3),
                color: alpha(theme.palette.primary.contrastText, 0.5),
              },
            }}
          >
            <CallIcon sx={{ fontSize: 28 }} />
          </IconButton>
        </motion.div>

        {/* Backspace */}
        <motion.div
          animate={{ opacity: dialInput ? 1 : 0 }}
          transition={{ duration: 0.15 }}
        >
          <IconButton
            onClick={handleBackspace}
            onDoubleClick={clearDialInput}
            tabIndex={dialInput ? 0 : -1}
            sx={{
              width: 48,
              height: 48,
              borderRadius: "14px",
              color: "text.secondary",
              "&:hover": {
                bgcolor: alpha(theme.palette.text.secondary, 0.08),
              },
            }}
          >
            <BackspaceOutlinedIcon sx={{ fontSize: 20 }} />
          </IconButton>
        </motion.div>
      </Box>
    </Box>
  );
}
