import { useState, useMemo, useCallback, useEffect, useRef } from "react";
import { Box, IconButton, alpha, useTheme, Typography, Autocomplete, TextField, Paper, Popper, Tooltip } from "@mui/material";
import CallIcon from "@mui/icons-material/Call";
import BackspaceOutlinedIcon from "@mui/icons-material/BackspaceOutlined";
import CancelRoundedIcon from "@mui/icons-material/CancelRounded";
import { motion, AnimatePresence } from "framer-motion";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { placeCall } from "../../hooks/useSip";
import { dialUri } from "../../utils/sipUri";
import { contactForNumber, dialerMatches, formatNumber, uriUser } from "../../utils/callerDisplay";
import type { Contact } from "../../types/sip";
import { DialerButton } from "./DialerButton";
import { parsePhoneNumber, type CountryCode, getCountries, getCountryCallingCode } from "libphonenumber-js";
import { useFitFont } from "../../hooks/useFitFont";
import { dialHint, formatAsYouType } from "../../utils/phoneFormat";

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
  const defaultCountry = useAppStore((s) => s.defaultCountry) as CountryCode;
  const setDefaultCountry = useAppStore((s) => s.setDefaultCountry);
  const theme = useTheme();

  const accounts = useAppStore((s) => s.accounts);
  const activeAccountId = useAppStore((s) => s.activeAccountId);
  const contacts = useAppStore((s) => s.contacts);
  // Letters mean a name: the box searches contacts instead of dialling.
  const typingName = /[a-z]/i.test(dialInput);
  const matches = useMemo(() => dialerMatches(dialInput, contacts), [dialInput, contacts]);
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

  // Display only: what is dialled is the digits as typed (`dialDigits`), so
  // an extension or a star code is never changed by the formatting.
  const formattedNumber = useMemo(() => formatAsYouType(dialInput, defaultCountry), [dialInput, defaultCountry]);
  const hint = useMemo(() => dialHint(dialInput, defaultCountry), [dialInput, defaultCountry]);
  const numberRef = useRef<HTMLDivElement>(null);
  useFitFont(numberRef, formattedNumber || t("dialer.enterNumber"), 35, 16);

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
  
  // Determine if we should show the country selector (hide for extensions and when auto-country code is disabled)
  const digits = dialInput.replace(/\D/g, "");
  const isInternational = dialInput.startsWith("+");
  const autoCountryCode = activeAccount?.autoCountryCode ?? false;
  const showCountrySelector = autoCountryCode
    ? (!dialInput || isInternational || digits.length > 6)
    : isInternational;

  const callContact = useCallback(
    async (contact: Contact) => {
      if (!activeAccountId || !activeAccount) return;
      await placeCall({
        uri: dialUri(uriUser(contact.uri) || contact.phone || "", activeAccount, defaultCountry),
        accountId: activeAccountId,
        remoteName: contact.name,
      }).catch(() => {});
      clearDialInput();
    },
    [activeAccountId, activeAccount, defaultCountry, clearDialInput],
  );

  const handleCall = useCallback(async () => {
    if (!dialInput.trim() || !activeAccountId || !activeAccount) return;
    // A name in the box calls the best match; no match, no call.
    if (typingName) {
      if (matches[0]) await callContact(matches[0]);
      return;
    }
    const uri = dialUri(dialInput, activeAccount, defaultCountry);

    // If the call cannot be placed, placeCall removes it and tells the user
    // in the app toast; nothing more to do here.
    await placeCall({
      uri,
      accountId: activeAccountId,
      // The contact's name when the number is one we know; otherwise the
      // call screen formats the number itself.
      remoteName: contactForNumber(dialInput, contacts)?.name,
    }).catch(() => {});
  }, [dialInput, activeAccountId, activeAccount, defaultCountry, typingName, matches, callContact, contacts]);

  // Read the store, not the render's dialInput: two deletes before a
  // re-render must remove two characters.
  const handleBackspace = useCallback(() => {
    setDialInput(useAppStore.getState().dialInput.slice(0, -1));
  }, [setDialInput]);

  const backspaceTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const startHoldToClear = useCallback(() => {
    if (backspaceTimerRef.current) clearTimeout(backspaceTimerRef.current);
    backspaceTimerRef.current = setTimeout(() => {
      clearDialInput();
      backspaceTimerRef.current = null;
    }, 450);
  }, [clearDialInput]);

  const stopHoldToClear = useCallback(() => {
    if (backspaceTimerRef.current) {
      clearTimeout(backspaceTimerRef.current);
      backspaceTimerRef.current = null;
    }
  }, []);

  useEffect(() => {
    return () => {
      if (backspaceTimerRef.current) clearTimeout(backspaceTimerRef.current);
    };
  }, []);

  // Native paste covers Cmd+V on macOS and Ctrl+V on Windows/Linux.
  useEffect(() => {
    const handlePaste = (event: ClipboardEvent) => {
      const target = event.target;
      if (
        event.defaultPrevented ||
        (target instanceof HTMLElement &&
          (target.closest("input, textarea, select") || target.isContentEditable))
      ) return;

      const text = event.clipboardData?.getData("text/plain").trim();
      if (!text) return;

      const normalized = text.replace(/[\u0660-\u0669\u06f0-\u06f9]/g, (digit) =>
        String(digit.charCodeAt(0) - (digit.charCodeAt(0) <= 0x0669 ? 0x0660 : 0x06f0))
      );
      // Reject prose, lists and multiline content rather than joining unrelated numbers.
      if (!/^\+?[0-9*#() \u00a0\u202f.-]+$/.test(normalized)) return;
      const number = normalized.replace(/[() \u00a0\u202f.-]/g, "");
      if (!/[0-9*#]/.test(number)) return;

      event.preventDefault();
      setDialInput(number);
    };

    window.addEventListener("paste", handlePaste);
    return () => window.removeEventListener("paste", handlePaste);
  }, [setDialInput]);

  // Keyboard input support
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // Leave native shortcuts and editable controls to their own handlers.
      if (
        e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey ||
        (e.target instanceof HTMLElement &&
          (e.target.closest("input, textarea, select") || e.target.isContentEditable))
      ) {
        return;
      }

      const key = e.key;

      // Digits 0-9
      if (/^[0-9]$/.test(key)) {
        appendDigit(key);
        return;
      }

      // Letters search contacts by name (a space between first and last).
      if (/^[a-zA-Z]$/.test(key) || (key === " " && /[a-z]/i.test(dialInput))) {
        e.preventDefault();
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
        // Key size follows the window: the keypad and the call button must
        // never be pushed below the bottom edge; the number area gives first.
        "--dial-key": "76px",
        "@media (max-height: 760px)": { "--dial-key": "68px" },
        "@media (max-height: 680px)": { "--dial-key": "60px" },
        height: "100%",
        minHeight: 0,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "flex-end",
        px: 3,
        pb: 2,
        pt: 1,
        boxSizing: "border-box",
      }}
    >
      {/* Number display with country flag */}
      <Box
        sx={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "flex-end",
          width: "100%",
          minHeight: 48,
          overflow: "visible",
          pb: 1,
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
                    ...params.slotProps,
                    input: {
                      ...params.slotProps.input,
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
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  setCountryPickerOpen(true);
                }
              }}
              role="button"
              tabIndex={0}
              aria-label={t("dialer.searchCountry")}
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

        <Box
          sx={{
            position: "relative",
            width: "100%",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          {/* The number is updated in place. It used to be keyed on its value
              inside AnimatePresence mode="wait", which must finish the old
              element's exit before mounting the new one; two changes in the
              same tick (a click and a double-click on Delete) left the exit
              stuck and the display frozen on a stale number (#80). */}
          <Box sx={{ flex: 1, minWidth: 0, textAlign: "center" }}>
              <Box
                ref={numberRef}
                data-testid="dial-number"
                sx={{
                  // The size is set by useFitFont: as large as fits, never
                  // below 16px; past that the number scrolls sideways.
                  fontSize: "2.2rem",
                  fontWeight: 200,
                  fontFamily: '"Google Sans", sans-serif',
                  color: dialInput ? "text.primary" : "text.secondary",
                  letterSpacing: dialInput ? "0.04em" : "0",
                  px: 1,
                  whiteSpace: "nowrap",
                  overflowX: "auto",
                  overflowY: "hidden",
                  scrollbarWidth: "none",
                  "&::-webkit-scrollbar": { display: "none" },
                  maxWidth: "100%",
                }}
              >
                {formattedNumber || t("dialer.enterNumber")}
              </Box>
              {hint && (
                <Box
                  data-testid="dial-hint"
                  sx={{ mt: 0.25, fontSize: "0.8rem", color: "text.secondary", opacity: 0.8, letterSpacing: "0.02em" }}
                >
                  {hint}
                </Box>
              )}
          </Box>
          {matches.length > 0 && (
            <Box
              role="listbox"
              aria-label={t("dialer.matches")}
              sx={{
                // Floats under the number, over the keypad: picking a contact
                // is the whole point of typing a name, and the keys below are
                // not needed until it is dismissed.
                position: "absolute",
                top: "100%",
                left: 8,
                right: 8,
                zIndex: 3,
                mt: 0.5,
                borderRadius: "14px",
                bgcolor: "background.paper",
                border: `1px solid ${alpha(theme.palette.divider, 0.3)}`,
                boxShadow: `0 8px 24px ${alpha("#000", theme.palette.mode === "dark" ? 0.5 : 0.15)}`,
                overflow: "hidden",
              }}
            >
              {matches.map((contact, i) => (
                <Box
                  key={contact.id}
                  role="option"
                  aria-selected={typingName && i === 0}
                  tabIndex={0}
                  onClick={() => void callContact(contact)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      void callContact(contact);
                    }
                  }}
                  sx={{
                    display: "flex",
                    alignItems: "center",
                    gap: 1.25,
                    px: 1.5,
                    py: 0.75,
                    cursor: "pointer",
                    bgcolor: typingName && i === 0 ? alpha(theme.palette.primary.main, 0.1) : "transparent",
                    "&:hover": { bgcolor: alpha(theme.palette.primary.main, 0.14) },
                  }}
                >
                  <Box
                    sx={{
                      width: 30,
                      height: 30,
                      borderRadius: "50%",
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "center",
                      bgcolor: alpha(theme.palette.primary.main, 0.14),
                      color: "primary.main",
                      fontSize: "0.75rem",
                      fontWeight: 600,
                      flexShrink: 0,
                    }}
                  >
                    {contact.name.substring(0, 2).toUpperCase()}
                  </Box>
                  <Box sx={{ minWidth: 0, textAlign: "left" }}>
                    <Typography variant="body2" noWrap sx={{ fontWeight: 500 }}>
                      {contact.name}
                    </Typography>
                    <Typography variant="caption" noWrap sx={{ color: "text.secondary", display: "block" }}>
                      {formatNumber(uriUser(contact.uri) || contact.phone || "", defaultCountry)}
                    </Typography>
                  </Box>
                </Box>
              ))}
            </Box>
          )}

          {/* A fixed slot at the end of the row, so the button sits beside the number and never over it. */}
          <Box sx={{ width: 32, flexShrink: 0, display: "flex", justifyContent: "center", order: 2 }}>
            <AnimatePresence>
              {dialInput && (
                <motion.div
                  initial={{ opacity: 0, scale: 0.7 }}
                  animate={{ opacity: 1, scale: 1 }}
                  exit={{ opacity: 0, scale: 0.7 }}
                  transition={{ duration: 0.15 }}
                  style={{ display: "flex" }}
                >
                  <Tooltip title={t("dialer.clearAll", "Clear")}>
                    <IconButton
                      onClick={clearDialInput}
                      size="small"
                      aria-label={t("dialer.clearAll", "Clear")}
                      sx={{
                        color: "text.secondary",
                        p: 0.5,
                        "&:hover": {
                          color: "text.primary",
                          bgcolor: alpha(theme.palette.text.secondary, 0.08),
                        },
                      }}
                    >
                      <CancelRoundedIcon sx={{ fontSize: 20 }} />
                    </IconButton>
                  </Tooltip>
                </motion.div>
              )}
            </AnimatePresence>
          </Box>
          {/* Mirrors the slot so the number stays centred. */}
          <Box sx={{ width: 32, flexShrink: 0, order: -1 }} />
        </Box>
      </Box>

      {/* Keypad grid */}
      <Box
        sx={{
          display: "grid",
          gridTemplateColumns: "repeat(3, var(--dial-key))",
          gap: 1.5,
          justifyContent: "center",
          mb: 2,
          flexShrink: 0,
        }}
      >
        {keys.map(({ digit, letters }) => (
          <DialerButton
            key={digit}
            digit={digit}
            letters={letters || undefined}
            onPress={appendDigit}
            onLongPress={digit === "0" ? () => appendDigit("+") : undefined}
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
            aria-label={t("dialer.call")}
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
          <Tooltip title={t("dialer.backspaceHold", "Delete (hold to clear)")}>
            <span>
              <IconButton
                onClick={handleBackspace}
                onMouseDown={startHoldToClear}
                onMouseUp={stopHoldToClear}
                onMouseLeave={stopHoldToClear}
                onTouchStart={startHoldToClear}
                onTouchEnd={stopHoldToClear}
                onTouchCancel={stopHoldToClear}
                disabled={!dialInput}
                tabIndex={dialInput ? 0 : -1}
                aria-label={t("dialer.backspaceHold", "Delete (hold to clear)")}
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
            </span>
          </Tooltip>
        </motion.div>
      </Box>
    </Box>
  );
}
