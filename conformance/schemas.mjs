import { readFileSync } from "node:fs";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";

const SCHEMA_FIELDS = ["inputSchema", "outputSchema"];

const { tools } = JSON.parse(readFileSync(0, "utf8")).result;

const problems = tools.length === 0 ? ["tools/list answered with no tools"] : tools.flatMap(schemaProblems);

problems.forEach((problem) => console.error(problem));
process.exit(problems.length === 0 ? 0 : 1);

function schemaProblems(tool) {
  return SCHEMA_FIELDS.filter((field) => field in tool).flatMap((field) =>
    compileProblems(`${tool.name}.${field}`, tool[field]),
  );
}

function compileProblems(where, schema) {
  const ajv = addFormats(new Ajv2020({ strict: true, allErrors: true }));
  try {
    ajv.compile(schema);
    return [];
  } catch (error) {
    return [`${where}: ${error.message}`];
  }
}
