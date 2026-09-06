package mn.shop.api;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.IOException;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.lang.reflect.RecordComponent;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.List;
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

    private static List<SchemaField> orderFields() throws IOException {
        String schema = read("api/src/main/resources/graphql/order.graphqls");
        Matcher body = ORDER_TYPE.matcher(schema);
        assertTrue(body.find(), "the schema no longer declares a type Order");
        List<SchemaField> fields = new ArrayList<>();
        Matcher field = SCHEMA_FIELD.matcher(body.group(1));
        while (field.find()) {
            fields.add(new SchemaField(field.group(1), field.group(2)));
        }
        assertFalse(fields.isEmpty(), "type Order in the schema declares no fields at all");
        return fields;
    }

    /** The fields the client's GraphQL query asks for, aliases resolved to the real name. */
    private static Set<String> selectedFields() throws IOException {
        String client = read("web/src/lib/orders.ts");
        Matcher selection = ORDERS_SELECTION.matcher(client);
        assertTrue(
                selection.find(),
                "web/src/lib/orders.ts no longer sends a query selecting fields on `orders`");
        Set<String> selected = new LinkedHashSet<>();
        // `total: grossAmount` selects grossAmount under an alias; drop the label, keep the field.
        Matcher token = Pattern.compile("(\\w+)\\s*:\\s*|(\\w+)").matcher(selection.group(1));
        while (token.find()) {
            if (token.group(2) != null) {
                selected.add(token.group(2));
            }
        }
        return selected;
    }

    @Test
    void everyFieldTheSchemaDeclaresIsActuallyServed() throws IOException {
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
    void everyFieldTheClientSelectsIsDeclaredByTheSchema() throws IOException {
        Set<String> declared = new LinkedHashSet<>();
        orderFields().forEach(f -> declared.add(f.name()));

        List<String> unknown = selectedFields().stream().filter(f -> !declared.contains(f)).toList();

        assertTrue(
                unknown.isEmpty(),
                "the orders query selects "
                        + unknown
                        + ", which type Order in the schema does not declare (it declares "
                        + declared
                        + ") — the seam still disagrees, in the other direction");
    }

    @Test
    void theAmountIsStillDeclaredSelectedAndNamedByTheView() throws IOException {
        List<String> amounts =
                orderFields().stream().filter(f -> f.type().startsWith("Float")).map(SchemaField::name).toList();
        assertFalse(
                amounts.isEmpty(),
                "type Order no longer declares any numeric field — the total the page renders was "
                        + "removed rather than repaired");

        Set<String> selected = selectedFields();
        // The client type and the component: the query literal is dropped so that selecting a
        // field is not mistaken for the client knowing about it.
        String clientCode = read("web/src/lib/orders.ts").replaceAll("(?s)`[^`]*`", " ");
        String view = read("web/src/components/OrderSummary.tsx");

        for (String amount : amounts) {
            assertTrue(
                    selected.contains(amount),
                    "the schema declares Order." + amount + " but the orders query never selects it");
            assertTrue(
                    Pattern.compile("\\b" + Pattern.quote(amount) + "\\b").matcher(clientCode + view).find(),
                    "the query selects "
                            + amount
                            + ", but neither the client type in orders.ts nor OrderSummary.tsx names "
                            + "it — the page reads a property the response does not carry");
        }
    }
}
