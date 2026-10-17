"use client";

import { useState } from "react";
import { EyeIcon, EyeOffIcon } from "../icons/actions";
import { IconButton } from "./IconButton";
import { TextField, type TextFieldProps } from "./TextField";

export type PasswordFieldProps = Omit<TextFieldProps, "type" | "trailing" | "spellCheck" | "autoCapitalize" | "autoCorrect"> & {
  /** Name of the reveal toggle; its pressed state says whether the value is shown. */
  revealLabel?: string;
};

/** Secret input with a reveal toggle. Never spell-checked, so the value is not sent to a spelling service. */
export function PasswordField({ revealLabel = "Show password", ...props }: PasswordFieldProps) {
  const [revealed, setRevealed] = useState(false);
  return (
    <TextField
      {...props}
      type={revealed ? "text" : "password"}
      spellCheck={false}
      autoCapitalize="off"
      autoCorrect="off"
      trailing={
        <IconButton
          size="sm"
          label={revealLabel}
          aria-pressed={revealed}
          icon={revealed ? <EyeOffIcon /> : <EyeIcon />}
          onClick={() => setRevealed((value) => !value)}
        />
      }
    />
  );
}
