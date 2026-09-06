package mn.shop.api;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.IOException;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.lang.reflect.RecordComponent;
import java.net.URL;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.stream.Stream;
import org.junit.jupiter.api.Test;

/**
 * L1 for B2-orphaned-field-diagnosis.
 *
 * <p>The task: "The orders page shows NaN for every total. Find out why and fix it."
 *
 * <p>The prompt names no file and no field. The cause is that the Java side was renamed and the
 * schema was not, so the schema declares a field nothing serves; the query selects it, the
 * server answers nothing for it, and {@code undefined.toFixed(2)} is the NaN. The graded end
 * state is therefore <em>agreement across the whole chain</em> and never a particular name:
 * finishing the rename forward and reverting the Java side backward are both correct, and a
 * test that named a field would grade which one the agent guessed.
 *
 * <p>This is one file rather than a Java half and a TypeScript half because the disagreement
 * spans both trees and neither side can see the other. It lives on the Java side because the
 * type the API publishes is worth reading by reflection rather than by regex — that is what a
 * resolver actually returns. The two frontend files are read from the repository root, found by
 * walking up rather than assumed, because Maven runs this test with {@code api/} as the working
 * directory.
 *
 * <p>A GraphQL alias is a fix, not a workaround: `totalAmount: grossAmount` renames the field in
 * the response, so a renamed schema and an untouched component agree. The selection set is
 * therefore read as a map from the schema-side name to the key the response carries, and the two
 * are checked against the two different things they describe.
 *
 * <p>Note what is deliberately <em>not</em> asserted here. That the component reads only fields
 * the client-side type declares is {@code tsc --noEmit}'s job and it does it at L0. That the old
 * name is gone everywhere is B1's business, not this task's: reverting Java to {@code
 * totalAmount} is a legitimate fix and leaves the old name in play by design.
 */
class HiddenTest {

    private static final Pattern ORDER_TYPE =
            Pattern.compile("type\\s+Order\\s*\\{([^}]*)\\}", Pattern.DOTALL);

    /** `name: Type!` or `name(args): Type!` inside a GraphQL type body. */
    private static final Pattern SCHEMA_FIELD =
            Pattern.compile("(?m)^\\s*(\\w+)\\s*(?:\\([^)]*\\))?\\s*:\\s*([\\w!\\[\\]]+)");

    /** The selection set of the `orders` field in the query the client sends. */
    private static final Pattern ORDERS_SELECTION =
            Pattern.compile("\\borders\\s*\\{([^{}]*)\\}", Pattern.DOTALL);

    /** One entry in a selection set: `grossAmount`, or `totalAmount: grossAmount`. */
    private static final Pattern SELECTED =
            Pattern.compile("(?:(\\w+)\\s*:\\s*)?(\\w+)");

    /** A field resolver: `@SchemaMapping` / `@BatchMapping`, with or without attributes. */
    private static final Pattern FIELD_RESOLVER =
            Pattern.compile(
                    "@(?:Schema|Batch)Mapping(?:\\s*\\(([^)]*)\\))?[^;{]*?\\b(\\w+)\\s*\\(",
                    Pattern.DOTALL);

    private static final Pattern STRING_LITERAL = Pattern.compile("\"([^\"]*)\"");

    private static final Set<String> OBJECT_METHODS = Set.of("toString", "hashCode", "clone");

    /**
     * The repository root: the nearest ancestor holding both trees. Walking up rather than using
     * `..` means this works whether the runner starts in `api/` or at the root.
     */
    private static Path repoRoot() {
        Path candidate = Path.of("").toAbsolutePath();
        while (candidate != null) {
            if (Files.isDirectory(candidate.resolve("api"))
                    && Files.isDirectory(candidate.resolve("web"))) {
                return candidate;
            }
            candidate = candidate.getParent();
        }
        throw new IllegalStateException(
                "no ancestor of " + Path.of("").toAbsolutePath() + " holds both api/ and web/");
    }

    private static String read(String relative) throws IOException {
        return Files.readString(repoRoot().resolve(relative));
    }

    private static String decapitalize(String s) {
        return Character.toLowerCase(s.charAt(0)) + s.substring(1);
    }

    /** The property names a caller of {@code type} can reach — see B1 for the same helper. */
    private static Set<String> publishedNames(Class<?> type) {
        Set<String> names = new LinkedHashSet<>();

        if (type.isRecord()) {
            for (RecordComponent component : type.getRecordComponents()) {
                names.add(component.getName());
            }
        }
        for (Field field : type.getDeclaredFields()) {
            if (!field.isSynthetic() && !Modifier.isStatic(field.getModifiers())) {
                names.add(field.getName());
            }
        }
        for (Method method : type.getDeclaredMethods()) {
            if (method.isSynthetic()
                    || method.getParameterCount() != 0
                    || Modifier.isStatic(method.getModifiers())
                    || OBJECT_METHODS.contains(method.getName())) {
                continue;
            }
            String name = method.getName();
            if (name.startsWith("get") && name.length() > 3) {
                names.add(decapitalize(name.substring(3)));
            } else if (name.startsWith("is") && name.length() > 2) {
                names.add(decapitalize(name.substring(2)));
            } else {
                names.add(name);
            }
        }
        return names;
    }

    /**
     * Field names an explicit resolver serves. A fix that keeps the schema and adds a resolver
     * for the orphaned field is a working fix, so it must not be graded red for choosing a
     * different shape than renaming.
     */
    private static Set<String> resolverServedNames() throws IOException {
        Set<String> served = new LinkedHashSet<>();
        Path sources = repoRoot().resolve("api/src/main/java");
        if (!Files.isDirectory(sources)) {
            return served;
        }
        try (Stream<Path> files = Files.walk(sources)) {
            for (Path file : files.filter(Files::isRegularFile).sorted().toList()) {
                if (!file.getFileName().toString().endsWith(".java")) {
                    continue;
                }
                Matcher resolver = FIELD_RESOLVER.matcher(Files.readString(file));
                while (resolver.find()) {
                    served.add(resolver.group(2)); // the method name
                    if (resolver.group(1) != null) {
                        Matcher named = STRING_LITERAL.matcher(resolver.group(1));
                        while (named.find()) {
                            served.add(named.group(1)); // @SchemaMapping(field = "x")
                        }
                    }
                }
            }
        }
        return served;
    }

    private record SchemaField(String name, String type) {}

    /**
     * Every schema file the application would load, concatenated — read off the test classpath
     * rather than from a path, so a fix that renames, moves or splits the schema file is still
     * graded on what it declares. A hard-coded path would throw rather than assert, and an
     * exception is not a red test.
     */
    private static String schema() throws Exception {
        URL location = HiddenTest.class.getResource("/graphql");
        assertNotNull(location, "no graphql/ schema directory on the classpath any more");
        try (Stream<Path> files = Files.walk(Path.of(location.toURI()))) {
            StringBuilder all = new StringBuilder();
            for (Path file : files.filter(Files::isRegularFile).sorted().toList()) {
                if (file.getFileName().toString().endsWith(".graphqls")) {
                    all.append(Files.readString(file)).append('\n');
                }
            }
            return all.toString();
        }
    }

    private static List<SchemaField> orderFields() throws Exception {
        Matcher body = ORDER_TYPE.matcher(schema());
        assertTrue(body.find(), "the schema no longer declares a type Order");
        List<SchemaField> fields = new ArrayList<>();
        Matcher field = SCHEMA_FIELD.matcher(body.group(1));
        while (field.find()) {
            fields.add(new SchemaField(field.group(1), field.group(2)));
        }
        assertFalse(fields.isEmpty(), "type Order in the schema declares no fields at all");
        return fields;
    }

    /**
     * The query's selection set: schema field name -> the key the response carries it under.
     * `totalAmount: grossAmount` selects the schema's `grossAmount` and hands the client back a
     * `totalAmount`, so both names matter and neither can be dropped — the schema side is what
     * the schema must declare, the client side is what the page reads.
     */
    private static Map<String, String> selectedFields() throws IOException {
        String client = read("web/src/lib/orders.ts");
        Matcher selection = ORDERS_SELECTION.matcher(client);
        assertTrue(
                selection.find(),
                "web/src/lib/orders.ts no longer sends a query selecting fields on `orders`");
        Map<String, String> selected = new LinkedHashMap<>();
        Matcher token = SELECTED.matcher(selection.group(1));
        while (token.find()) {
            String alias = token.group(1);
            String field = token.group(2);
            selected.put(field, alias != null ? alias : field);
        }
        return selected;
    }

    @Test
    void everyFieldTheSchemaDeclaresIsActuallyServed() throws Exception {
        Set<String> answerable = new LinkedHashSet<>(publishedNames(OrderDto.class));
        answerable.addAll(resolverServedNames());

        List<String> orphaned =
                orderFields().stream().map(SchemaField::name).filter(f -> !answerable.contains(f)).toList();

        assertTrue(
                orphaned.isEmpty(),
                "the GraphQL schema declares "
                        + orphaned
                        + " on type Order, but nothing serves those fields — the API publishes "
                        + answerable
                        + ". A query selecting them gets no value back, which is the undefined the "
                        + "orders page renders as NaN");
    }

    @Test
    void everyFieldTheClientSelectsIsDeclaredByTheSchema() throws Exception {
        Set<String> declared = new LinkedHashSet<>();
        orderFields().forEach(f -> declared.add(f.name()));

        List<String> unknown =
                selectedFields().keySet().stream().filter(f -> !declared.contains(f)).toList();

        assertTrue(
                unknown.isEmpty(),
                "the orders query selects "
                        + unknown
                        + ", which type Order in the schema does not declare (it declares "
                        + declared
                        + ") — the seam still disagrees, in the other direction");
    }

    @Test
    void theAmountIsStillDeclaredSelectedAndNamedByTheView() throws Exception {
        List<String> amounts =
                orderFields().stream().filter(f -> f.type().startsWith("Float")).map(SchemaField::name).toList();
        assertFalse(
                amounts.isEmpty(),
                "type Order no longer declares any numeric field — the total the page renders was "
                        + "removed rather than repaired");

        Map<String, String> selected = selectedFields();
        // The client type and the component: the query literal is dropped so that selecting a
        // field is not mistaken for the client knowing about it.
        String clientCode = read("web/src/lib/orders.ts").replaceAll("(?s)`[^`]*`", " ");
        String view = read("web/src/components/OrderSummary.tsx");

        for (String amount : amounts) {
            assertTrue(
                    selected.containsKey(amount),
                    "the schema declares Order." + amount + " but the orders query never selects it");
            // What the page reads is the key the *response* carries, which an alias renames. A
            // query that selects grossAmount as `totalAmount: grossAmount` and a component that
            // reads totalAmount agree perfectly; demanding the schema-side name here would grade
            // a working fix red for choosing an alias over an edit.
            String carriedAs = selected.get(amount);
            assertTrue(
                    Pattern.compile("\\b" + Pattern.quote(carriedAs) + "\\b").matcher(clientCode + view).find(),
                    "the query selects "
                            + amount
                            + (carriedAs.equals(amount) ? "" : " as `" + carriedAs + "`")
                            + ", but neither the client type in orders.ts nor OrderSummary.tsx names "
                            + carriedAs
                            + " — the page reads a property the response does not carry");
        }
    }
}
