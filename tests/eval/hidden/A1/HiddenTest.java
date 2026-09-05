package mn.pay;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.IOException;
import java.math.BigDecimal;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.stream.Stream;
import org.junit.jupiter.api.Test;

/**
 * L1 for A1-idempotency-key-length.
 *
 * <p>The task: "The idempotency key column is too short for the new upstream provider. Widen it to
 * 128 characters everywhere it is constrained."
 *
 * <p>The observable behaviour is a single sentence: <em>a 128-character idempotency key fits</em>.
 * Every assertion below is a place that can refuse one — the schema, the entity, the validator —
 * and each is phrased as "does not cap below 128" rather than "equals 128", because the task says
 * widen, not set. Nothing here asserts <em>how</em> the widening was expressed: editing V1 in place
 * and appending an ALTER migration both pass, VARCHAR(128) and VARCHAR (256) both pass, and a
 * validator that never checks length at all passes exactly like one whose limit is now 128.
 *
 * <p>Two of the three assertions can pass vacuously at the start commit — the entity and the
 * validator carry no length constraint there, so there is nothing to widen in them. That is the
 * honest reading of "everywhere it is constrained", and it is deliberate: this file grades whether
 * a 128-character key now fits, and {@code required_sites} separately grades whether the agent
 * looked in all three places. An L3 failure beside an L1 pass is information about the agent's
 * search, not a defect in this test.
 */
class HiddenTest {

    private static final Path MIGRATIONS = Path.of("src/main/resources/db/migration");

    /** A width declared for the idempotency key column: `idempotency_key ... VARCHAR(n)`. */
    private static final Pattern DECLARED_WIDTH =
            Pattern.compile("idempotency_key\\b[^;()]*?varchar\\s*\\(\\s*(\\d+)\\s*\\)");

    /** The `@Column(...)` attached to the idempotency key field, whatever the entity is called. */
    private static final Pattern IDEMPOTENCY_COLUMN =
            Pattern.compile("@Column\\s*\\(([^)]*)\\)[^;]*?\\bidempotencyKey\\b");

    private static final Pattern LENGTH_ATTRIBUTE = Pattern.compile("length\\s*=\\s*(\\d+)");

    /** SQL comments carry the words the parsing below looks for. Remove them before reading. */
    private static String stripSqlComments(String sql) {
        return sql.replaceAll("(?s)/\\*.*?\\*/", " ").replaceAll("(?m)--[^\\n]*", " ");
    }

    private static List<Path> filesUnder(Path root, String suffix) throws IOException {
        try (Stream<Path> s = Files.walk(root)) {
            return s.filter(Files::isRegularFile)
                    .filter(p -> p.getFileName().toString().endsWith(suffix))
                    .sorted()
                    .toList();
        }
    }

    @Test
    void theSchemaLetsA128CharacterKeyFit() throws IOException {
        boolean columnExists = false;
        Integer effectiveWidth = null; // migrations apply in order, so the last declaration wins

        for (Path migration : filesUnder(MIGRATIONS, ".sql")) {
            String sql = stripSqlComments(Files.readString(migration)).toLowerCase(Locale.ROOT);
            if (sql.contains("idempotency_key")) {
                columnExists = true;
            }
            Matcher m = DECLARED_WIDTH.matcher(sql);
            while (m.find()) {
                effectiveWidth = Integer.parseInt(m.group(1));
            }
        }

        assertTrue(columnExists, "the migrations must still declare an idempotency_key column");
        // No width at all means an unbounded type (TEXT, VARCHAR with no length) — also wide enough.
        assertTrue(
                effectiveWidth == null || effectiveWidth >= 128,
                "after every migration runs the idempotency key column is still capped at "
                        + effectiveWidth
                        + " characters, so a 128-character key from the new provider would not fit");
    }

    @Test
    void theEntityDoesNotCapTheKeyBelow128() throws IOException {
        List<String> tooNarrow = new ArrayList<>();

        for (Path source : filesUnder(Path.of("src/main/java"), ".java")) {
            Matcher column = IDEMPOTENCY_COLUMN.matcher(Files.readString(source));
            while (column.find()) {
                Matcher length = LENGTH_ATTRIBUTE.matcher(column.group(1));
                if (length.find() && Integer.parseInt(length.group(1)) < 128) {
                    tooNarrow.add(source + " declares length = " + length.group(1));
                }
            }
        }

        // Silent when the mapping declares no length: JPA's own default is wider than 128, so an
        // unannotated column is not a cap. This assertion exists to catch a fix that widens the
        // schema and leaves — or introduces — a narrower cap in the mapping.
        assertTrue(
                tooNarrow.isEmpty(),
                "the JPA mapping caps the idempotency key below 128 characters: " + tooNarrow);
    }

    @Test
    void theValidatorAcceptsAKeyOfTheNewLength() {
        String key = "k".repeat(128);
        assertDoesNotThrow(
                () -> new PaymentValidator().check(key, new BigDecimal("10.00")),
                "a 128-character idempotency key must be accepted by validation");
    }
}
