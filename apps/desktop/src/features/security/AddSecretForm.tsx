import { useRef, useState, type FormEvent } from "react";
import { KeyIcon, PlusIcon } from "@aidex/ui/icons";
import { Button, Card, Grid, PasswordField, Select, Stack, TextField } from "@aidex/ui/primitives";
import { useToast } from "../../app/providers/toast";
import { sampleWorkspaces } from "../../lib/sample/workspaces";

const SECRET_NAME_PATTERN = /^[A-Za-z][A-Za-z0-9_-]*$/;
const GLOBAL_SCOPE = "all";

const scopeOptions = [
  { value: GLOBAL_SCOPE, label: "All workspaces" },
  ...sampleWorkspaces.map((workspace) => ({ value: workspace.id, label: workspace.name })),
];

type Errors = { name?: string; value?: string };

function validate(name: string, value: string): Errors {
  const errors: Errors = {};
  if (!name.trim()) errors.name = "Enter a name, e.g. ANTHROPIC_API_KEY.";
  else if (!SECRET_NAME_PATTERN.test(name.trim())) errors.name = "Use letters, digits, hyphens, and underscores only.";
  if (!value) errors.value = "Enter the secret value.";
  return errors;
}

/**
 * Inline form for a new vault secret. There is no vault yet, so submitting only validates and
 * clears the value from state; the value is never stored, logged, or sent anywhere.
 */
export function AddSecretForm() {
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [scope, setScope] = useState(GLOBAL_SCOPE);
  const [errors, setErrors] = useState<Errors>({});
  const nameRef = useRef<HTMLInputElement>(null);
  const valueRef = useRef<HTMLInputElement>(null);
  const notify = useToast();

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const nextErrors = validate(name, value);
    setErrors(nextErrors);
    if (nextErrors.name) return nameRef.current?.focus();
    if (nextErrors.value) return valueRef.current?.focus();

    const scopeLabel = scopeOptions.find((option) => option.value === scope)?.label ?? "All workspaces";
    setValue("");
    setName("");
    notify(`Preview: ${name.trim()} would be encrypted into the vault for ${scopeLabel} once AiDEX Core is connected. The value was discarded.`);
  };

  return (
    <Card padding="md">
      <form onSubmit={handleSubmit} noValidate aria-label="Add secret">
        <Stack gap={5}>
          <Grid columns={3} gap={4}>
            <TextField
              ref={nameRef}
              label="Name"
              placeholder="ANTHROPIC_API_KEY"
              leadingIcon={<KeyIcon />}
              autoComplete="off"
              spellCheck={false}
              value={name}
              error={errors.name}
              onChange={(event) => {
                setName(event.target.value);
                setErrors((current) => ({ ...current, name: undefined }));
              }}
            />
            <PasswordField
              ref={valueRef}
              label="Value"
              placeholder="Paste the secret"
              revealLabel="Show secret value"
              autoComplete="off"
              value={value}
              error={errors.value}
              onChange={(event) => {
                setValue(event.target.value);
                setErrors((current) => ({ ...current, value: undefined }));
              }}
            />
            <Select label="Scope" options={scopeOptions} value={scope} onChange={(event) => setScope(event.target.value)} />
          </Grid>
          <Stack direction="row" justify="end">
            <Button type="submit" variant="primary" size="md" icon={<PlusIcon />}>
              Add secret
            </Button>
          </Stack>
        </Stack>
      </form>
    </Card>
  );
}
