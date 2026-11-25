import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { MealSlot } from "./MealSlot";

describe("MealSlot", () => {
  it("toggles between metric and cups", () => {
    render(<MealSlot meal="lunch" recipe="Pancakes" />);
    fireEvent.click(screen.getByText("Show cups"));
    expect(screen.getByText("Show grams")).toBeTruthy();
  });
});
