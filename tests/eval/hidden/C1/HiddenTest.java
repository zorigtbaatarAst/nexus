package mn.payments;

import static org.junit.jupiter.api.Assertions.assertFalse;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.stream.Stream;
import org.junit.jupiter.api.Test;

/**
 * L1 for C1-regression-recognised.
 *
 * <p>The task: "Payments are being double-charged again in production. Find the cause and fix it."
 *
 * <p>The application-level idempotency check races; the database uniqueness constraint is what
 * actually stops two concurrent inserts of the same key. A migration dropped it. So the observable
 * behaviour is the end state of the migration set: <em>after every migration has run, in order, the
 * database still refuses a second payment carrying an idempotency key it has already seen</em>.
 *
 * <p>This asserts that end state and nothing about which file changed. Deleting the offending DROP,
 * deleting the whole migration that contains it, and appending a new migration that recreates the
 * constraint all pass. So does {@code CREATE UNIQUE INDEX}, {@code ALTER TABLE ... ADD CONSTRAINT
 * ... UNIQUE}, and an inline {@code UNIQUE} on the column, under any name.
 */
class HiddenTest {

    private static final Path MIGRATIONS = Path.of("src/main/resources/db/migration");

    /** The name a statement gives to the index or constraint it creates or drops. */
    private static final Pattern OBJECT_NAME =
            Pattern.compile("\\b(?:index|constraint)\\s+(?:if\\s+(?:not\\s+)?exists\\s+)?([a-z0-9_\"]+)");

    /**
     * A uniqueness source with no name of its own — {@code idempotency_key VARCHAR(64) NOT NULL
     * UNIQUE} inside CREATE TABLE. Not a legal SQL identifier, so no DROP can accidentally match it.
     */
    private static final String UNNAMED = "<inline unique>";

    private static String stripSqlComments(String sql) {
        return sql.replaceAll("(?s)/\\*.*?\\*/", " ").replaceAll("(?m)--[^\\n]*", " ");
    }

    private static List<Path> migrations() throws IOException {
        try (Stream<Path> s = Files.walk(MIGRATIONS)) {
            return s.filter(Files::isRegularFile)
                    .filter(p -> p.getFileName().toString().endsWith(".sql"))
                    .sorted()
                    .toList();
        }
    }

    /**
     * Whole-identifier match, not substring: `DROP INDEX ux_payment_idempotency_key` must not be
     * read as dropping a live constraint named `ux_payment_idempotency`. Underscores are word
     * characters, so the boundary falls where SQL's identifier boundary does.
     */
    private static boolean mentions(String statement, String name) {
        return Pattern.compile("\\b" + Pattern.quote(name) + "\\b").matcher(statement).find();
    }

    private static String nameIn(String statement) {
        Matcher m = OBJECT_NAME.matcher(statement);
        return m.find() ? m.group(1).replace("\"", "") : UNNAMED;
    }

    /**
     * Replays the migration set and returns the uniqueness constraints on the idempotency key that
     * are still standing at the end. Tracking names rather than a single flag is what lets a DROP of
     * one index leave a differently-named constraint alone.
     */
    private static Set<String> liveUniquenessConstraints() throws IOException {
        Set<String> live = new LinkedHashSet<>();

        for (Path migration : migrations()) {
            String sql = stripSqlComments(Files.readString(migration)).toLowerCase(Locale.ROOT);
            for (String statement : sql.split(";")) {
                if (!statement.contains("idempotency")) {
                    continue;
                }
                // Removal first, then addition, and both are plain `if`s: one statement can do
                // both. `ALTER TABLE ... DROP CONSTRAINT IF EXISTS x, ADD CONSTRAINT x UNIQUE (...)`
                // is the standard idempotent-migration idiom and restores the constraint; an
                // `else` here would grade it as a drop and fail a correct fix over its SQL idiom.
                //
                // "drop" alone would also match `ALTER COLUMN ... DROP NOT NULL`, which removes
                // no uniqueness at all. A drop only counts if it names a schema object.
                if (statement.contains("drop")
                        && (statement.contains("index") || statement.contains("constraint"))) {
                    live.removeIf(name -> mentions(statement, name));
                }
                if (statement.contains("unique")) {
                    live.add(nameIn(statement));
                }
            }
        }
        return live;
    }

    @Test
    void theDatabaseStillRefusesASecondPaymentWithTheSameIdempotencyKey() throws IOException {
        assertFalse(
                liveUniquenessConstraints().isEmpty(),
                "after every migration has run there is no uniqueness constraint left on the "
                        + "payment idempotency key, so two concurrent requests carrying the same key "
                        + "can both insert — the double charge is still reachable");
    }
}
