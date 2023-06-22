import { APIContext, APIRoute, EndpointOutput } from "astro";
import data from "../../../data/output.json";
import { makeBadge } from "badge-maker";

export const getStaticPaths = async () => {
  return data.users.slice(0, 100).map((user, index) => ({
    params: { name: user.login },
    props: { user, index },
  }));
};

export const get: APIRoute = async (context: APIContext) => {
  const badge = makeBadge({
    label: "NZ Committers Rank",
    message: `#${context.props.index + 1}`,
    color: "#3faf44",
    labelColor: "#555",
    style: "for-the-badge",
  });

  return new Response(badge, {
    status: 200,
    headers: { "Content-Type": "image/svg+xml" },
  });
};
