package mn.pay;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.math.BigDecimal;
import java.util.Locale;
import java.util.regex.Pattern;
import org.junit.jupiter.api.Test;

/**
 * L1 for N1-null-task.
 *
 * <p>The task: "PaymentValidator's error message says 'amount must be positive'. Change it to
 * 'amount must be greater than zero'."
 *
 * <p>The observable behaviour is one sentence: <em>an amount the validator refuses is refused in
 * the new wording</em>. Both assertions run the validator rather than read its source, so nothing
 * here cares whether the new text is a literal in place, a named constant, or a message a helper
 * builds with the offending amount appended.
 *
 * <p>The assertion is a containment and a ban, never an equality: a message that <em>contains</em>
 * "greater than zero" passes however much context it adds around it, and "greater than 0" passes
 * too, because a fix that spells the number is still the wording the prompt dictates. What fails
 * is a message that still carries "must be positive" — the prompt says change it to, not mention
 * as well, and leaving the old phrase standing is the half-fix this file exists to catch. Case
 * and run-of-whitespace differences are normalised away for the same reason.
 *
 * <p>A third test asserts that a valid amount is still <em>accepted</em>. It is vacuous at the start
 * commit and stays vacuous under every correct fix, and it is not optional: without it a validator
 * that throws the new wording at every input — valid amounts included — passes both assertions
 * above, and nothing else in this build would catch it. The project's own test constructs
 * {@code new PaymentService(null, null)} and never reaches the validator at all.
 *
 * <p>Two amounts are refused, zero and negative, because they are the two the phrase "must be
 * positive" describes and a fix that reworks the branch structure can update one and forget the
 * other. The <em>null</em> amount is deliberately not asserted on: it shares the message at the
 * start commit only because it shares the branch, and a fix that splits it out under a message of
 * its own ("amount is required") has still done what the task asks. Grading that red would grade
 * whether the agent refactored the way we would have.
 *
 * <p>Nothing here reads the exception type either — {@code assertThrows(Exception.class, ...)}
 * accepts whatever the validator throws, so a fix that introduces its own exception class is
 * graded on the wording, which is what the task is about.
 *
 * <p>Compiles against {@code new PaymentValidator()} and
 * {@code PaymentValidator.check(String, BigDecimal)}: a message change has no reason to touch
 * either, so a compile error here is a signal about the agent's diff rather than a defect in this
 * test. That is the same accidental coupling A1 carries against the same method.
 */
class HiddenTest {

    /** The wording the prompt dictates. "0" is accepted for the same reason: it is the number. */
    private static final Pattern NEW_WORDING = Pattern.compile("greater than (?:zero|0)");

    /** The wording the prompt says to replace. Still present anywhere in the message is a fail. */
    private static final Pattern OLD_WORDING = Pattern.compile("must be positive");

    /** A valid key, so the amount check is the one that speaks. */
    private static final String KEY = "idem-key-1";

    /** The message the validator refuses {@code amount} with, lower-cased and space-collapsed. */
    private static String rejectionMessageFor(BigDecimal amount) {
        Exception refusal =
                assertThrows(
                        Exception.class,
                        () -> new PaymentValidator().check(KEY, amount),
                        "the validator accepted an amount of " + amount + " instead of refusing it");
        String message = refusal.getMessage() == null ? "" : refusal.getMessage();
        return message.toLowerCase(Locale.ROOT).replaceAll("\\s+", " ").trim();
    }

    private static void assertRefusedInTheNewWording(BigDecimal amount) {
        String message = rejectionMessageFor(amount);
        assertTrue(
                NEW_WORDING.matcher(message).find(),
                "an amount of "
                        + amount
                        + " is refused with \""
                        + message
                        + "\", which does not say the amount must be greater than zero");
        assertTrue(
                !OLD_WORDING.matcher(message).find(),
                "an amount of "
                        + amount
                        + " is refused with \""
                        + message
                        + "\", which still carries the old wording \"must be positive\"");
    }

    @Test
    void aZeroAmountIsRefusedInTheNewWording() {
        assertRefusedInTheNewWording(BigDecimal.ZERO);
    }

    @Test
    void aNegativeAmountIsRefusedInTheNewWording() {
        assertRefusedInTheNewWording(new BigDecimal("-1.00"));
    }

    /**
     * The other half of "refused in the new wording": a valid amount is still accepted. Without
     * this, a {@code check} that throws the new message at everything is green — the project's own
     * test never constructs a validator, so nothing else in the build would notice.
     */
    @Test
    void aValidAmountIsStillAccepted() {
        assertDoesNotThrow(
                () -> new PaymentValidator().check(KEY, new BigDecimal("10.00")),
                "the validator now refuses a valid amount of 10.00; the message changed but so did"
                        + " what passes validation");
    }
}
