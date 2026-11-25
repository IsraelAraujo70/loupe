import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { IngredientList } from "./IngredientList";

describe("IngredientList", () => {
  it("says when there is nothing", () => {
    render(<IngredientList lines={[]} />);
    expect(screen.getByText("No ingredients yet.")).toBeTruthy();
  });
});
