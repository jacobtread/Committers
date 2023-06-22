import { APIContext, APIRoute, EndpointOutput } from "astro";
import { makeBadge } from "badge-maker";

export const get: APIRoute = async (context: APIContext) => {
  const badge = makeBadge({
    label: "NZ Committers Rank",
    message: "Not ranked",
    color: "#da3333",
    labelColor: "#555",
    style: "for-the-badge",
  });

  return new Response(badge, {
    status: 200,
    headers: { "Content-Type": "image/svg+xml" },
  });
};
