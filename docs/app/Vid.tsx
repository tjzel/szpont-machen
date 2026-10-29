"use client";

import { useEffect, useState } from "react";

export default function Vid({ name, alt }: { name: string; alt: string }) {
  const [safari, setSafari] = useState<boolean | null>(null);

  useEffect(() => {
    setSafari(
      /^((?!chrome|chromium|android).)*safari/i.test(navigator.userAgent),
    );
  }, []);

  return (
    <div
      className="vid"
      style={{ backgroundImage: `url(/szpont-machen/vids/${name}.png)` }}
      role="img"
      aria-label={alt}
    >
      {safari === null ? null : safari ? (
        <video autoPlay muted loop playsInline aria-label={alt}>
          <source
            src={`/szpont-machen/vids/${name}-hevc.mp4`}
            type="video/mp4"
          />
        </video>
      ) : (
        <img src={`/szpont-machen/vids/${name}.webp`} alt={alt} />
      )}
    </div>
  );
}
