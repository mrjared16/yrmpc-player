
import sys

file_path = "rmpc/src/ui/panes/search_pane_v2.rs"

with open(file_path, "r") as f:
    lines = f.readlines()

# The pattern we want to replace starts around line 997 and ends around line 1050
# We want to replace the manual grouping logic with the new simplified version

start_pattern = "                let items: Vec<DetailItem> = data.into_iter().map(DetailItem::from).collect();"
end_pattern = "                self.phase = Phase::BrowseResults;"

start_idx = -1
end_idx = -1

for i, line in enumerate(lines):
    if start_pattern in line:
        start_idx = i
    if end_pattern in line and start_idx != -1 and i > start_idx:
        end_idx = i
        break

if start_idx != -1 and end_idx != -1:
    # Keep the start line
    new_lines = lines[:start_idx+1]

    # Add the simplified implementation
    new_lines.append("\n")
    new_lines.append("                // Clear stack and set new root - ContentDetails::into_sections handles grouping\n")
    new_lines.append("                self.view.clear();\n")
    new_lines.append("                self.view.push(SearchableContent::results(\"Results\", items));\n")
    new_lines.append("\n")

    # Add the end line and rest of file
    new_lines.append(lines[end_idx])
    new_lines.extend(lines[end_idx+1:])

    with open(file_path, "w") as f:
        f.writelines(new_lines)
    print("Successfully updated search_pane_v2.rs")
else:
    print(f"Could not find patterns. Start: {start_idx}, End: {end_idx}")
