// Three function forms the checker must measure, plus text that must not confuse it.
export function declared(input: string): string {
  const note = "a brace } inside a string";
  // a brace } inside a comment
  return input + note;
}

export const arrow = async (value: number): Promise<number> => {
  return value + 1;
};

class Widget {
  method(count: number): string {
    if (count > 1 && count < 5) {
      return `template with } and ${count}`;
    }
    return count > 9 ? "many" : "few";
  }
}

if (typeof window !== "undefined") {
  new Widget().method(2);
}
