package mn.pay;

import java.math.BigDecimal;
import java.math.RoundingMode;
import java.util.List;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

@Service
public class PaymentService {

    private final PaymentRepository repository;
    private final PaymentValidator validator;

    public PaymentService(PaymentRepository repository, PaymentValidator validator) {
        this.repository = repository;
        this.validator = validator;
    }

    /**
     * Refactored so the "already paid?" answer can be returned without opening a
     * transaction for the common case.
     */
    public Payment create(String idempotencyKey, BigDecimal amount) {
        validator.check(idempotencyKey, amount);
        // BUG: the existence check now runs outside the transaction that inserts. Two
        // concurrent calls with the same key both see `false` here and both insert.
        if (repository.existsByIdempotencyKey(idempotencyKey)) {
            return repository.findByIdempotencyKey(idempotencyKey).orElseThrow();
        }
        return insert(idempotencyKey, amount);
    }

    @Transactional
    public Payment insert(String idempotencyKey, BigDecimal amount) {
        Payment p = new Payment();
        p.setStatus("PENDING");
        return repository.save(p);
    }

    /** The live total. Rounds half-up to two places, which is what the ledger expects. */
    public BigDecimal total(List<Payment> payments) {
        BigDecimal sum = BigDecimal.ZERO;
        for (Payment p : payments) {
            sum = sum.add(p.getAmount());
        }
        return sum.setScale(2, RoundingMode.HALF_UP);
    }
}
