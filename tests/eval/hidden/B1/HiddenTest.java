package mn.shop.api;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.lang.reflect.RecordComponent;
import java.net.URL;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.LinkedHashSet;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.stream.Stream;
import org.junit.jupiter.api.Test;

/**
 * L1 for B1-rename-crosses-the-seam, backend half.
 *
 * <p>The task: "Rename the {@code totalAmount} field on Order to {@code grossAmount}
 * throughout."
 *
 * <p>Neither compiler in this repository can see the rename. {@code mvn test} is green at the
 * start commit and green after a rename that touches only the frontend; {@code tsc} is green
 * either way too. So the Java side of the rename is not covered by L0 at all, and asserting it
 * needs its own file. See {@code hidden.test.ts} beside this one for the frontend half.
 *
 * <p>What is asserted is the <em>published contract</em> of the two types, not their source
 * text: the set of property names a caller can reach — record components, declared fields and
 * no-argument accessors, unioned. That is what Spring GraphQL maps onto a schema field and what
 * a serializer would emit. Reading it by reflection rather than by regex means a fix that
 * changes shape — record to class, field plus getter to a bare accessor — is graded on what it
 * publishes rather than on how it is written.
 *
 * <p>"Throughout" is taken at its word: a leftover {@code getTotalAmount()} beside a renamed
 * field is a public API still carrying the old name, and it is the exact half-rename this
 * corpus plants as a bug at the next commit. It is graded red on purpose.
 *
 * <p>The schema is read off the test classpath rather than from a path, so nothing here assumes
 * where the file sits or what it is called — every {@code .graphqls} under {@code classpath:
 * graphql/}, which is where Spring GraphQL itself looks.
 */
class HiddenTest {

    /** A field declaration inside a GraphQL type body: `name: Type!`. */
    private static final Pattern SCHEMA_FIELD =
            Pattern.compile("(?m)^\\s*(\\w+)\\s*(?:\\([^)]*\\))?\\s*:");

    private static final Pattern ORDER_TYPE =
            Pattern.compile("type\\s+Order\\s*\\{([^}]*)\\}", Pattern.DOTALL);

    /** Names every object declares; they are not properties of the domain type. */
    private static final Set<String> OBJECT_METHODS = Set.of("toString", "hashCode", "clone");

    private static String decapitalize(String s) {
        return Character.toLowerCase(s.charAt(0)) + s.substring(1);
    }

    /**
     * The property names a caller of {@code type} can reach. Union rather than a single source:
     * a record publishes components, an entity publishes fields and getters, and a rename that
     * updated only one of them has not renamed the field "throughout".
     */
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
                names.add(name); // record-style accessor: grossAmount()
            }
        }

        return names;
    }

    private static void assertRenamed(Class<?> type) {
        Set<String> published = publishedNames(type);
        assertFalse(
                published.contains("totalAmount"),
                type.getSimpleName()
                        + " still publishes totalAmount (it publishes "
                        + published
                        + "), so the old name is still part of its API");
        assertTrue(
                published.contains("grossAmount"),
                type.getSimpleName()
                        + " publishes no grossAmount (it publishes "
                        + published
                        + "), so the renamed field never reached this type");
    }

    /** Every schema file the application would load, concatenated. */
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

    private static Set<String> orderFieldsDeclaredBy(String schema) {
        Matcher body = ORDER_TYPE.matcher(schema);
        assertTrue(body.find(), "the schema no longer declares a type Order");
        Set<String> fields = new LinkedHashSet<>();
        Matcher field = SCHEMA_FIELD.matcher(body.group(1));
        while (field.find()) {
            fields.add(field.group(1));
        }
        return fields;
    }

    @Test
    void theEntityPublishesTheNewName() {
        assertRenamed(Order.class);
    }

    @Test
    void theDtoPublishesTheNewName() {
        assertRenamed(OrderDto.class);
    }

    @Test
    void theSchemaDeclaresTheNewName() throws Exception {
        Set<String> declared = orderFieldsDeclaredBy(schema());
        assertFalse(
                declared.contains("totalAmount"),
                "the GraphQL schema still declares Order.totalAmount (it declares "
                        + declared
                        + "), so every client generated from it keeps the old name");
        assertTrue(
                declared.contains("grossAmount"),
                "the GraphQL schema declares no Order.grossAmount (it declares "
                        + declared
                        + "), so the renamed field is not part of the published contract");
    }
}
