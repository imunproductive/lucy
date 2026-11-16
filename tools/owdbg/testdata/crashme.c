/* Test target for tools/owdbg (build: gcc -O1 -o crashme.exe crashme.c).
 *   crashme loop N    call work() N times (100 ms apart) on the main thread and on a new thread
 *                     per 10 calls; work() reads and writes g_counter
 *   crashme crash     a handled first-chance exception (vectored handler), then a NULL write in
 *                     level3() (unhandled: second chance)
 * Functions and g_counter are exported so owdbg names them without symbols. */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>

__declspec(dllexport) volatile long long g_counter[2];

__declspec(dllexport) __attribute__((noinline)) long long work(long long i)
{
    g_counter[0] += i;
    return g_counter[0] * 2 + g_counter[1];
}

static DWORD WINAPI worker(LPVOID p)
{
    return (DWORD)work((long long)(INT_PTR)p);
}

static LONG CALLBACK veh(PEXCEPTION_POINTERS info)
{
    return info->ExceptionRecord->ExceptionCode == 0xE0000001 ? EXCEPTION_CONTINUE_EXECUTION
                                                              : EXCEPTION_CONTINUE_SEARCH;
}

__declspec(dllexport) __attribute__((noinline)) void level3(volatile int *p)
{
    *p = 0x1234;
}

__declspec(dllexport) __attribute__((noinline)) void level2(volatile int *p)
{
    level3(p);
    g_counter[1]++;
}

__declspec(dllexport) __attribute__((noinline)) void level1(volatile int *p)
{
    level2(p);
    g_counter[1]++;
}

int main(int argc, char **argv)
{
    if (argc >= 2 && !strcmp(argv[1], "crash")) {
        AddVectoredExceptionHandler(1, veh);
        RaiseException(0xE0000001, 0, 0, NULL);
        printf("handled first-chance exception\n");
        fflush(stdout);
        level1(NULL);
        return 0;
    }
    int n = argc >= 3 ? atoi(argv[2]) : 50;
    long long sum = 0;
    for (int i = 0; i < n; i++) {
        sum += work(i);
        if (i % 10 == 0) {
            HANDLE t = CreateThread(NULL, 0, worker, (LPVOID)(INT_PTR)i, 0, NULL);
            WaitForSingleObject(t, INFINITE);
            CloseHandle(t);
        }
        Sleep(100);
    }
    printf("sum %lld\n", sum);
    return 0;
}
