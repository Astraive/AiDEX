"use client";

import { Menu as BaseMenu } from "@base-ui/react/menu";
import {
  createContext,
  useContext,
  useLayoutEffect,
  useState,
  type ComponentPropsWithRef,
  type ReactNode,
} from "react";
import { CheckIcon, ChevronRightIcon } from "../icons/interface";
import { cx } from "../utils/cx";
import { Kbd } from "./Kbd";
import { LAYER_ATTRIBUTE, pushLayer } from "./layers";
import styles from "./Menu.module.css";

/** Whether the nearest menu (or submenu) is open, so its popup can register as a layer. */
const MenuOpenContext = createContext(false);

export type MenuProps = BaseMenu.Root.Props;

/**
 * A menu of actions (Base UI Menu): arrow keys, Home/End and typeahead move the highlight, Enter
 * or Space picks, Escape and Tab close it, and focus returns to the trigger. It is modal: pointer
 * input outside only closes it and never reaches what lies underneath.
 *
 * Compose `MenuTrigger` (use `render` to make an IconButton or Button the trigger) and `MenuPopup`
 * with `MenuItem`, `MenuGroup`, `MenuGroupLabel`, `MenuSeparator`, `MenuCheckboxItem`,
 * `MenuRadioGroup` / `MenuRadioItem` and `MenuSub` parts.
 */
export function Menu({ open, defaultOpen = false, onOpenChange, ...props }: MenuProps) {
  const [openState, setOpenState] = useState(defaultOpen);
  return (
    <MenuOpenContext.Provider value={open ?? openState}>
      <BaseMenu.Root
        {...props}
        open={open}
        defaultOpen={defaultOpen}
        onOpenChange={(next, details) => {
          onOpenChange?.(next, details);
          if (!details.isCanceled) setOpenState(next);
        }}
      />
    </MenuOpenContext.Provider>
  );
}

export type MenuTriggerProps = BaseMenu.Trigger.Props;

/** Opens the menu on press, or on ArrowDown / ArrowUp with the first or last item highlighted. */
export function MenuTrigger(props: MenuTriggerProps) {
  return <BaseMenu.Trigger {...props} />;
}

export type MenuPopupProps = Omit<BaseMenu.Popup.Props, "className"> & {
  className?: string;
  /** Preferred side of the trigger or anchor (default `bottom`); it flips when there is no room. */
  side?: BaseMenu.Positioner.Props["side"];
  align?: BaseMenu.Positioner.Props["align"];
  /** Gap to the trigger in pixels (default 4). */
  sideOffset?: number;
  alignOffset?: number;
  /** Positions against another element than the trigger, e.g. when the menu has no `MenuTrigger`. */
  anchor?: BaseMenu.Positioner.Props["anchor"];
  /** Keeps the popup mounted (hidden) while closed. */
  keepMounted?: boolean;
};

/** The floating list. It never grows past the viewport and scrolls when it would. */
export function MenuPopup({
  className,
  side = "bottom",
  align = "start",
  sideOffset = 4,
  alignOffset,
  anchor,
  keepMounted = false,
  children,
  ...popupProps
}: MenuPopupProps) {
  const open = useContext(MenuOpenContext);
  const [portal, setPortal] = useState<HTMLDivElement | null>(null);
  // Register while open, so a popover underneath leaves Escape and outside presses to this menu.
  useLayoutEffect(() => {
    if (!open || !portal) return;
    return pushLayer(portal);
  }, [open, portal]);
  return (
    <BaseMenu.Portal ref={setPortal} keepMounted={keepMounted} className={styles.portal} {...{ [LAYER_ATTRIBUTE]: "" }}>
      <BaseMenu.Positioner
        className={styles.positioner}
        side={side}
        align={align}
        sideOffset={sideOffset}
        alignOffset={alignOffset}
        anchor={anchor}
        collisionPadding={8}
      >
        <BaseMenu.Popup {...popupProps} className={cx(styles.popup, className)}>
          {children}
        </BaseMenu.Popup>
      </BaseMenu.Positioner>
    </BaseMenu.Portal>
  );
}

export type MenuItemProps = Omit<BaseMenu.Item.Props, "className"> & {
  className?: string;
  /** A 16px icon before the label. */
  icon?: ReactNode;
  /** A keyboard shortcut shown at the end, e.g. "Ctrl N". */
  shortcut?: ReactNode;
  tone?: "default" | "danger";
};

/**
 * An action. A disabled item stays focusable and announced (with `aria-disabled`), so a reason can
 * be shown for it, but it cannot be picked.
 */
export function MenuItem({ className, icon, shortcut, tone = "default", children, ...props }: MenuItemProps) {
  return (
    <BaseMenu.Item {...props} className={cx(styles.item, tone === "danger" && styles.danger, className)}>
      {icon ? (
        <span className={styles.icon} aria-hidden="true">
          {icon}
        </span>
      ) : null}
      <span className={styles.label}>{children}</span>
      {shortcut ? <MenuShortcut>{shortcut}</MenuShortcut> : null}
    </BaseMenu.Item>
  );
}

export type MenuCheckboxItemProps = Omit<BaseMenu.CheckboxItem.Props, "className"> & { className?: string };

/** A toggle with a check mark in front. */
export function MenuCheckboxItem({ className, children, ...props }: MenuCheckboxItemProps) {
  return (
    <BaseMenu.CheckboxItem {...props} className={cx(styles.item, styles.choice, className)}>
      <BaseMenu.CheckboxItemIndicator className={styles.indicator} keepMounted>
        <CheckIcon />
      </BaseMenu.CheckboxItemIndicator>
      <span className={styles.label}>{children}</span>
    </BaseMenu.CheckboxItem>
  );
}

export type MenuRadioGroupProps = BaseMenu.RadioGroup.Props;

/** A set of `MenuRadioItem`s with one value. */
export function MenuRadioGroup(props: MenuRadioGroupProps) {
  return <BaseMenu.RadioGroup {...props} />;
}

export type MenuRadioItemProps = Omit<BaseMenu.RadioItem.Props, "className"> & { className?: string };

/** One choice in a `MenuRadioGroup`, with a check mark in front when chosen. */
export function MenuRadioItem({ className, children, ...props }: MenuRadioItemProps) {
  return (
    <BaseMenu.RadioItem {...props} className={cx(styles.item, styles.choice, className)}>
      <BaseMenu.RadioItemIndicator className={styles.indicator} keepMounted>
        <CheckIcon />
      </BaseMenu.RadioItemIndicator>
      <span className={styles.label}>{children}</span>
    </BaseMenu.RadioItem>
  );
}

export type MenuGroupProps = BaseMenu.Group.Props;

/** Groups related items; name it with a `MenuGroupLabel` inside. */
export function MenuGroup(props: MenuGroupProps) {
  return <BaseMenu.Group {...props} />;
}

export type MenuGroupLabelProps = Omit<BaseMenu.GroupLabel.Props, "className"> & { className?: string };

export function MenuGroupLabel({ className, ...props }: MenuGroupLabelProps) {
  return <BaseMenu.GroupLabel {...props} className={cx(styles.groupLabel, className)} />;
}

export type MenuSeparatorProps = { className?: string };

export function MenuSeparator({ className }: MenuSeparatorProps) {
  return <BaseMenu.Separator className={cx(styles.separator, className)} />;
}

export type MenuShortcutProps = ComponentPropsWithRef<"kbd">;

/** A shortcut hint at the end of an item; hidden with the other key hints when they are turned off. */
export function MenuShortcut({ className, ...props }: MenuShortcutProps) {
  return <Kbd className={cx(styles.shortcut, className)} {...props} />;
}

export type MenuSubProps = BaseMenu.SubmenuRoot.Props;

/** A nested menu: put a `MenuSubTrigger` and a `MenuSubPopup` inside. */
export function MenuSub({ open, defaultOpen = false, onOpenChange, ...props }: MenuSubProps) {
  const [openState, setOpenState] = useState(defaultOpen);
  return (
    <MenuOpenContext.Provider value={open ?? openState}>
      <BaseMenu.SubmenuRoot
        {...props}
        open={open}
        defaultOpen={defaultOpen}
        onOpenChange={(next, details) => {
          onOpenChange?.(next, details);
          if (!details.isCanceled) setOpenState(next);
        }}
      />
    </MenuOpenContext.Provider>
  );
}

export type MenuSubTriggerProps = Omit<BaseMenu.SubmenuTrigger.Props, "className"> & {
  className?: string;
  icon?: ReactNode;
};

/** The item that opens a submenu (ArrowRight, Enter, or hover), with a chevron at the end. */
export function MenuSubTrigger({ className, icon, children, ...props }: MenuSubTriggerProps) {
  return (
    <BaseMenu.SubmenuTrigger {...props} className={cx(styles.item, styles.subTrigger, className)}>
      {icon ? (
        <span className={styles.icon} aria-hidden="true">
          {icon}
        </span>
      ) : null}
      <span className={styles.label}>{children}</span>
      <ChevronRightIcon className={styles.chevron} aria-hidden="true" />
    </BaseMenu.SubmenuTrigger>
  );
}

export type MenuSubPopupProps = Omit<MenuPopupProps, "side">;

/** A submenu's list, beside its trigger; the offset lines its first item up with the trigger. */
export function MenuSubPopup({ sideOffset = 0, alignOffset = -5, ...props }: MenuSubPopupProps) {
  return <MenuPopup {...props} side="inline-end" sideOffset={sideOffset} alignOffset={alignOffset} />;
}
