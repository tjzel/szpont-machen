"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

const LINKS = [
  { href: "/", label: "Overview" },
  { href: "/usage", label: "Usage" },
  { href: "/keys", label: "TUI keys" },
  { href: "/features", label: "Features" },
  { href: "/limits", label: "Limits" },
  { href: "/mcp", label: "MCP" },
];

export default function Nav() {
  const pathname = usePathname();

  return (
    <nav>
      {LINKS.map(({ href, label }) => (
        <Link
          key={href}
          href={href}
          className={pathname === href ? "active" : ""}
        >
          {label}
        </Link>
      ))}
    </nav>
  );
}
